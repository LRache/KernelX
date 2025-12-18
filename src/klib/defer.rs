pub struct Defer<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

impl<F: FnOnce()> Defer<F> {
    fn cancel(&mut self) {
        self.0.take();
    }
}

pub fn defer<F: FnOnce()>(f: F) -> Defer<F> {
    Defer(Some(f))
}

pub fn cancel<F: FnOnce()>(mut defer: Defer<F>) {
    defer.cancel();
}
