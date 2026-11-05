//! Where the core's event loop meets Relm4's: what the core says, from whichever thread it says it,
//! arrives as a message to the window's component, in order, on the GTK thread.

use relm4::Sender;
use tern_app::view::{Effect, Update};
use tern_app::EngineListener;

#[derive(Debug)]
pub enum Incoming {
	Update(Box<Update>),
	Effect(Effect),
}

pub struct Bridge(pub Sender<Incoming>);

impl EngineListener for Bridge {
	fn on_update(&self, update: Update) {
		self.0.emit(Incoming::Update(Box::new(update)));
	}
	fn on_effect(&self, effect: Effect) {
		self.0.emit(Incoming::Effect(effect));
	}
	fn on_log(&self, _: String) {}
}
