use crate::report::Result;

pub struct Deferer<F: FnOnce() -> Result<()>> {
    f: Option<F>,
}

impl<F: FnOnce() -> Result<()>> Deferer<F> {
    pub fn new(f: F) -> Self {
        Self {
            f: Option::Some(f),
        }
    }
}

impl<F: FnOnce() -> Result<()>> Drop for Deferer<F> {
    fn drop(&mut self) {
        if let Option::Some(f) = self.f.take() {
            if let Result::Err(error) = f() {
                error.report();
            }
        }
    }
}
