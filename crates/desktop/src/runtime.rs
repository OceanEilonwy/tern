//! The core's Tokio runtime, which backend futures run on, and the bridge back to the GTK thread.

use std::future::Future;

/// Runs `fut` on the Tokio runtime; await the result from the GTK thread.
pub async fn run<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
	match tern_app::runtime().spawn(fut).await {
		Ok(v) => v,
		Err(e) => std::panic::resume_unwind(e.into_panic()),
	}
}

/// Runs `fut` on the GTK main loop, which is where widgets may be touched.
pub fn spawn_local(fut: impl Future<Output = ()> + 'static) {
	gtk::glib::spawn_future_local(fut);
}
