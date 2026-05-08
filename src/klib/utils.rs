use super::SpinLock;

pub struct LazyInitedCell<T> {
    value: SpinLock<Option<T>>,
}

impl<T: Clone> LazyInitedCell<T> {
    pub fn new(name: &'static str) -> Self {
        Self {
            value: SpinLock::new(None, name),
        }
    }

    pub fn get(&self) -> Option<T> {
        self.value.lock().clone()
    }

    pub fn get_or_init(&self, init: impl FnOnce() -> T) -> T {
        let mut value = self.value.lock();
        if let Some(value) = value.as_ref() {
            return value.clone();
        }

        let new_value = init();
        *value = Some(new_value.clone());
        new_value
    }
}
