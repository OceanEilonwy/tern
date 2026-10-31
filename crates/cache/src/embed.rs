//! Search by meaning: a static embedding model (model2vec) turns a text into a vector, so mails
//! about the same thing land close together even when they share no words.
//!
//! A static model is a table of one vector per word piece; a text's vector is the mean of its
//! pieces' vectors. No neural network runs, so it is pure Rust and takes microseconds per mail.
//! The tokenizer is BERT's (lower case, accents off, punctuation apart, longest word pieces first),
//! written out here for the one kind of model Tern loads instead of pulling in a tokenizer library.

use std::collections::HashMap;

/// Word pieces read per text, as model2vec does by default; the start of a mail says what it is about.
const MAX_TOKENS: usize = 512;
/// Longer words are unknown, as in BERT.
const MAX_WORD_CHARS: usize = 100;

pub struct Model {
	vocab: HashMap<String, u32>,
	unknown: Option<u32>,
	dim: usize,
	/// One row of `dim` numbers per word piece.
	table: Vec<f32>,
}

impl Model {
	/// Reads a model2vec model from its `tokenizer.json` and `model.safetensors`.
	pub fn load(tokenizer: &[u8], safetensors: &[u8]) -> Result<Model, String> {
		let json: serde_json::Value = serde_json::from_slice(tokenizer).map_err(|e| format!("tokenizer: {e}"))?;
		let model = &json["model"];
		if model["type"] != "WordPiece" || model["continuing_subword_prefix"] != "##" {
			return Err("tokenizer: not a WordPiece model".into());
		}
		let vocab: HashMap<String, u32> = model["vocab"]
			.as_object()
			.ok_or("tokenizer: no vocabulary")?
			.iter()
			.filter_map(|(piece, id)| Some((piece.clone(), u32::try_from(id.as_u64()?).ok()?)))
			.collect();
		let unknown = model["unk_token"].as_str().and_then(|t| vocab.get(t).copied());
		let (dim, table) = read_safetensors(safetensors)?;
		Self::new(vocab, unknown, dim, table)
	}

	/// A model from its parts: `table` holds a row of `dim` numbers for every id in `vocab`.
	pub fn new(vocab: HashMap<String, u32>, unknown: Option<u32>, dim: usize, table: Vec<f32>) -> Result<Model, String> {
		let rows = table.len().checked_div(dim).unwrap_or(0);
		if dim == 0 || !table.len().is_multiple_of(dim) || vocab.values().any(|&id| id as usize >= rows) {
			return Err("model: the vocabulary does not fit the table".into());
		}
		Ok(Model { vocab, unknown, dim, table })
	}

	pub fn dim(&self) -> usize {
		self.dim
	}

	/// The word pieces of `text`, by id, unknown ones left out.
	pub fn tokens(&self, text: &str) -> Vec<u32> {
		let mut ids = Vec::new();
		for word in words(text) {
			if ids.len() >= MAX_TOKENS {
				break;
			}
			self.pieces(&word, &mut ids);
		}
		ids.retain(|&id| Some(id) != self.unknown);
		ids.truncate(MAX_TOKENS);
		ids
	}

	/// Splits one word into the longest pieces the vocabulary has; a word that does not split is unknown.
	fn pieces(&self, word: &str, ids: &mut Vec<u32>) {
		let chars: Vec<(usize, char)> = word.char_indices().collect();
		if chars.len() > MAX_WORD_CHARS {
			ids.extend(self.unknown);
			return;
		}
		let mut found = Vec::new();
		let mut start = 0;
		while start < chars.len() {
			let from = chars[start].0;
			let piece = (start + 1..=chars.len()).rev().find_map(|end| {
				let to = chars.get(end).map_or(word.len(), |c| c.0);
				let piece = if start == 0 { word[from..to].to_string() } else { format!("##{}", &word[from..to]) };
				self.vocab.get(&piece).map(|&id| (id, end))
			});
			match piece {
				Some((id, end)) => {
					found.push(id);
					start = end;
				},
				None => {
					ids.extend(self.unknown);
					return;
				},
			}
		}
		ids.extend(found);
	}

	/// The meaning of `text` as a unit vector; all zeros when no word of it is known.
	pub fn embed(&self, text: &str) -> Vec<f32> {
		let tokens = self.tokens(text);
		let mut sum = vec![0f32; self.dim];
		for &id in &tokens {
			let row = &self.table[id as usize * self.dim..][..self.dim];
			for (s, v) in sum.iter_mut().zip(row) {
				*s += v;
			}
		}
		let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
		if norm > 0.0 {
			sum.iter_mut().for_each(|v| *v /= norm);
		}
		sum
	}
}

/// BERT's normalizing and splitting: lower case without accents, control characters gone, split at
/// spaces, with each punctuation mark and CJK character a word of its own.
fn words(text: &str) -> Vec<String> {
	let mut words = Vec::new();
	let mut word = String::new();
	let end = |word: &mut String, words: &mut Vec<String>| {
		if !word.is_empty() {
			words.push(std::mem::take(word));
		}
	};
	for c in text.chars() {
		// Control and format characters (soft hyphens, zero-width spaces) are dropped.
		if c == '\0' || c == '\u{fffd}' || (c.is_control() && !c.is_whitespace()) || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}') {
			continue;
		}
		if c.is_whitespace() {
			end(&mut word, &mut words);
			continue;
		}
		for c in c.to_lowercase() {
			let Some(c) = strip_accent(c) else { continue };
			if is_punctuation(c) || is_cjk(c) {
				end(&mut word, &mut words);
				words.push(c.to_string());
			} else {
				word.push(c);
			}
		}
	}
	end(&mut word, &mut words);
	words
}

/// A lower-case letter without its accent, as Unicode decomposition would leave it; `None` for a
/// combining mark on its own. Covers the Latin letters of European languages, which is what the
/// English model knows; others pass unchanged.
fn strip_accent(c: char) -> Option<char> {
	if ('\u{300}'..='\u{36f}').contains(&c) {
		return None;
	}
	if c.is_ascii() {
		return Some(c);
	}
	const FROM: &str = "àáâãäåçèéêëìíîïñòóôõöùúûüýÿāăąćĉċčďēĕėęěĝğġģĥĩīĭįĵķĺļľńņňōŏőŕŗřśŝşšţťũūŭůűųŵŷźżžơưǎǐǒǔǖǘǚǜǟǡǣǧǩǫǭǯǰǵǹǻǽǿȁȃȅȇȉȋȍȏȑȓȕȗșțȟȧȩȫȭȯȱȳ";
	const TO: &str = "aaaaaaceeeeiiiinooooouuuuyyaaaccccdeeeeegggghiiiijklllnnnooorrrssssttuuuuuuwyzzzouaiouuuuuaaægkooʒjgnaæøaaeeiioorruusthaeooooy";
	Some(FROM.chars().position(|f| f == c).and_then(|i| TO.chars().nth(i)).unwrap_or(c))
}

fn is_punctuation(c: char) -> bool {
	c.is_ascii_punctuation() || matches!(c, '¡' | '§' | '«' | '¶' | '·' | '»' | '¿' | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205e}' | '\u{3001}'..='\u{3003}' | '\u{3008}'..='\u{3011}')
}

fn is_cjk(c: char) -> bool {
	matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{20000}'..='\u{2a6df}' | '\u{2a700}'..='\u{2ceaf}' | '\u{f900}'..='\u{faff}' | '\u{2f800}'..='\u{2fa1f}')
}

/// The one table of a safetensors file of 32-bit floats: its row width and its numbers.
fn read_safetensors(raw: &[u8]) -> Result<(usize, Vec<f32>), String> {
	let bad = || "model: not a safetensors file".to_string();
	let len = u64::from_le_bytes(raw.get(..8).ok_or_else(bad)?.try_into().map_err(|_| bad())?) as usize;
	let header: serde_json::Value = serde_json::from_slice(raw.get(8..8usize.checked_add(len).ok_or_else(bad)?).ok_or_else(bad)?).map_err(|_| bad())?;
	let data = &raw[8 + len..];
	let table = header.as_object().and_then(|h| h.iter().find(|(k, _)| *k != "__metadata__")).map(|(_, v)| v).ok_or_else(bad)?;
	if table["dtype"] != "F32" {
		return Err("model: only 32-bit float models are supported".into());
	}
	let shape: Vec<usize> = table["shape"].as_array().ok_or_else(bad)?.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect();
	let offsets: Vec<usize> = table["data_offsets"].as_array().ok_or_else(bad)?.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect();
	let (&[rows, dim], &[start, end]) = (shape.as_slice(), offsets.as_slice()) else { return Err(bad()) };
	let bytes = data.get(start..end).filter(|b| b.len() == rows * dim * 4).ok_or_else(bad)?;
	Ok((dim, bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()))
}

/// A vector as stored: its largest magnitude as a little-endian float, then each number as a
/// signed byte of that scale. A quarter of the floats' size, so search reads a quarter of the
/// encrypted pages, and close enough for ranking (off by about 0.005 in a cosine).
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
	let scale = vector.iter().fold(0f32, |m, v| m.max(v.abs()));
	let mut blob = (scale / 127.0).to_le_bytes().to_vec();
	blob.extend(vector.iter().map(|v| if scale > 0.0 { (v / scale * 127.0).round() as i8 as u8 } else { 0 }));
	blob
}

/// The dot product of `vector` with one stored as a blob; the cosine, as both are unit vectors.
pub fn dot_blob(vector: &[f32], blob: &[u8]) -> f32 {
	let Some((scale, bytes)) = blob.split_first_chunk::<4>() else { return 0.0 };
	f32::from_le_bytes(*scale) * vector.iter().zip(bytes).map(|(a, &b)| a * (b as i8) as f32).sum::<f32>()
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;

	/// A model with a few words in two dimensions: food points one way, money the other.
	pub fn tiny() -> Model {
		let rows: [(&str, [f32; 2]); 9] = [
			("[UNK]", [0.0, 0.0]),
			("lunch", [1.0, 0.0]),
			("dinner", [0.9, 0.1]),
			("pizza", [0.95, 0.05]),
			("invoice", [0.0, 1.0]),
			("payment", [0.1, 0.9]),
			("pay", [0.2, 0.8]),
			("##ment", [0.0, 1.0]),
			(",", [0.5, 0.5]),
		];
		let vocab = rows.iter().enumerate().map(|(i, (w, _))| (w.to_string(), i as u32)).collect();
		Model::new(vocab, Some(0), 2, rows.iter().flat_map(|(_, v)| *v).collect()).unwrap()
	}

	#[test]
	fn words_are_split_like_bert() {
		assert_eq!(words("Héllo, WORLD!  naïve\tcafé 東京"), ["hello", ",", "world", "!", "naive", "cafe", "東", "京"]);
		assert_eq!(words("a\u{0}b\u{301}c"), ["abc"]);
		assert_eq!(words("mia@tuta.com"), ["mia", "@", "tuta", ".", "com"]);
	}

	#[test]
	fn word_pieces_take_the_longest_match_and_unknown_words_drop() {
		let m = tiny();
		// "payment" is whole; "paying" does not split ("##ing" is unknown), so it is dropped.
		assert_eq!(m.tokens("Payment, paying"), [5, 8]);
		assert_eq!(m.tokens("paym"), Vec::<u32>::new());
		assert_eq!(m.tokens(&"lunch ".repeat(600)).len(), MAX_TOKENS);
	}

	#[test]
	fn texts_about_the_same_thing_are_close() {
		let m = tiny();
		let lunch = m.embed("Pizza for dinner?");
		let money = m.embed("your payment");
		let query = m.embed("lunch");
		assert!((lunch.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
		assert!(dot_blob(&query, &to_blob(&lunch)) > 0.9);
		assert!(dot_blob(&query, &to_blob(&money)) < 0.3);
		assert_eq!(m.embed("nothing known"), [0.0, 0.0]);
		assert_eq!(dot_blob(&query, &to_blob(&[0.0, 0.0])), 0.0);
	}

	#[test]
	fn stored_vectors_keep_their_cosines() {
		let a: Vec<f32> = (0..256).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
		let b: Vec<f32> = (0..256).map(|i| ((i * 53 % 89) as f32 - 44.0) / 44.0).collect();
		let unit = |v: &[f32]| {
			let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
			v.iter().map(|x| x / n).collect::<Vec<f32>>()
		};
		let (a, b) = (unit(&a), unit(&b));
		let exact: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
		assert_eq!(to_blob(&a).len(), 4 + 256);
		assert!((dot_blob(&a, &to_blob(&b)) - exact).abs() < 0.01);
		assert!((dot_blob(&a, &to_blob(&a)) - 1.0).abs() < 0.01);
	}

	/// Against the real potion-base-8M: token ids as Hugging Face's own tokenizer gives them (unknown
	/// pieces dropped, as model2vec does). `TERN_MODEL_DIR=~/.cache/tern/models/potion-base-8M cargo test -p tern-cache -- --ignored`
	#[test]
	#[ignore]
	fn tokens_match_the_reference_tokenizer() {
		let dir = std::path::PathBuf::from(std::env::var_os("TERN_MODEL_DIR").expect("TERN_MODEL_DIR"));
		let model = Model::load(&std::fs::read(dir.join("tokenizer.json")).unwrap(), &std::fs::read(dir.join("model.safetensors")).unwrap()).unwrap();
		let cases: [(&str, &[u32]); 6] = [
			("Héllo, WORLD! naïve café 東京", &[6598, 16, 1094, 5, 14749, 6674, 885, 761]),
			("Your invoice for October is attached; total €19.00.", &[1121, 1005, 5773, 5616, 1011, 1261, 1009, 3993, 31, 1567, 580, 15153, 18, 3008, 18]),
			("Are you free for lunch on Friday? 12:30 at the usual place", &[1030, 1023, 1495, 1011, 5271, 1012, 4964, 35, 1266, 30, 1388, 1018, 1002, 4162, 1179]),
			("Grüße aus Köln — straße, Ærøskøbing, łódź", &[23671, 1232, 16505, 16157, 27312, 523, 1364, 26813, 16, 103, 1105, 15421, 5717, 15421, 9478, 16, 16820]),
			("mia@tuta.com https://example.org/help?x=1", &[7770, 36, 9728, 1702, 18, 3018, 15776, 30, 19, 19, 1748, 18, 7923, 19, 1399, 35, 66, 33, 21]),
			("unsubscribing\u{ad}from newsletters\u{200b} now", &[3901, 5348, 4916, 25781, 11328, 1081, 18705, 4364, 16184, 1021, 1091]),
		];
		for (text, ids) in cases {
			assert_eq!(model.tokens(text), ids, "{text}");
		}
	}

	#[test]
	fn a_safetensors_table_reads_back() {
		let header = br#"{"__metadata__":{"x":"y"},"embeddings":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
		let mut raw = (header.len() as u64).to_le_bytes().to_vec();
		raw.extend_from_slice(header);
		raw.extend([1.0f32, 2.0, 3.0, 4.0].iter().flat_map(|v| v.to_le_bytes()));
		assert_eq!(read_safetensors(&raw).unwrap(), (2, vec![1.0, 2.0, 3.0, 4.0]));
		assert!(read_safetensors(&raw[..raw.len() - 1]).is_err());
		assert!(read_safetensors(b"junk").is_err());
		let tokenizer = br###"{"model":{"type":"WordPiece","continuing_subword_prefix":"##","unk_token":"[UNK]","vocab":{"[UNK]":0,"hi":1}}}"###;
		let model = Model::load(tokenizer, &raw).unwrap();
		assert_eq!(model.embed("hi"), [0.6, 0.8]);
		// A vocabulary larger than the table is refused.
		let tokenizer = br###"{"model":{"type":"WordPiece","continuing_subword_prefix":"##","vocab":{"a":0,"b":1,"c":2}}}"###;
		assert!(Model::load(tokenizer, &raw).is_err());
	}
}
