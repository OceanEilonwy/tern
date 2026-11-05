//! Writes the Kotlin bindings from the compiled library; the Android build runs it.

fn main() {
	uniffi::uniffi_bindgen_main()
}
