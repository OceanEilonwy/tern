//! A handle on a pane's end edge: dragging it resizes the pane.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::graphene;

/// Puts `content` in an overlay with a handle on its end edge, and returns both.
pub fn wrap(content: &impl IsA<gtk::Widget>) -> (gtk::Overlay, gtk::Box) {
	let handle = gtk::Box::builder().width_request(6).halign(gtk::Align::End).css_classes(["pane-grip"]).build();
	handle.set_cursor_from_name(Some("col-resize"));
	let overlay = gtk::Overlay::builder().child(content).build();
	overlay.add_overlay(&handle);
	(overlay, handle)
}

/// Dragging `handle` calls `resize` with the width `pane` is dragged to, then `done`; a double click
/// calls `double`.
pub fn connect(handle: &gtk::Box, pane: &impl IsA<gtk::Widget>, resize: impl Fn(i32) + 'static, done: impl Fn() + 'static, double: impl Fn() + 'static) {
	// The handle moves as the pane follows the pointer, so the pointer is measured from the window.
	let window_x = |handle: &gtk::Box, (x, y): (f64, f64)| {
		let root = handle.root()?;
		handle.compute_point(&root, &graphene::Point::new(x as f32, y as f32)).map(|p| p.x() as f64)
	};
	let (start_x, start_width) = (Rc::new(Cell::new(0.0)), Rc::new(Cell::new(0)));
	let drag = gtk::GestureDrag::new();
	let (sx, sw, pane) = (start_x.clone(), start_width.clone(), pane.clone().upcast::<gtk::Widget>());
	drag.connect_drag_begin(move |g, x, y| {
		let handle = g.widget().and_downcast::<gtk::Box>().unwrap();
		sx.set(window_x(&handle, (x, y)).unwrap_or(0.0));
		sw.set(pane.width());
	});
	drag.connect_drag_update(move |g, _, _| {
		let handle = g.widget().and_downcast::<gtk::Box>().unwrap();
		let Some(now) = g.point(None).and_then(|p| window_x(&handle, p)) else { return };
		let moved = now - start_x.get();
		let moved = if handle.direction() == gtk::TextDirection::Rtl { -moved } else { moved };
		resize(start_width.get() + moved.round() as i32);
	});
	drag.connect_drag_end(move |_, _, _| done());
	handle.add_controller(drag);
	let click = gtk::GestureClick::new();
	click.connect_pressed(move |_, n, _, _| {
		if n == 2 {
			double();
		}
	});
	handle.add_controller(click);
}
