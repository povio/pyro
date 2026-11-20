use std::{cell::{Cell, RefCell}, rc::Rc};

thread_local! {
    static ACTIVE: RefCell<Option<Rc<Budget>>> = const { RefCell::new(None) };
}

pub struct Budget {
    pub left: Cell<u64>,
    pub exhausted: Cell<bool>,
}

impl Budget {
    pub fn take(&self, count: usize) -> bool {
        if let Some(left) = self.left.get().checked_sub(count as u64).filter(|_| !self.exhausted.get()) {
            self.left.set(left);
            return true;
        }
        self.exhausted.set(true);
        false
    }
}

pub fn scope<T>(work: &mut u64, run: impl FnOnce(&Budget) -> T) -> Option<T> {
    struct Restore(Option<Rc<Budget>>);
    impl Drop for Restore {
        fn drop(&mut self) { ACTIVE.replace(self.0.take()); }
    }
    let budget = Rc::new(Budget { left: Cell::new(*work), exhausted: Cell::new(false) });
    let _restore = Restore(ACTIVE.replace(Some(budget.clone())));
    let result = run(&budget);
    *work = if budget.exhausted.get() { 0 } else { budget.left.get() };
    (!budget.exhausted.get()).then_some(result)
}

pub fn charge(units: usize) -> bool {
    ACTIVE.with_borrow(|budget| budget.as_ref().is_none_or(|budget| budget.take(units)))
}

pub fn surface(width: u64, height: u64, layers: usize) -> bool {
    ACTIVE.with_borrow(|active| active.as_ref().is_none_or(|budget| {
        let pixels = width.saturating_mul(height).saturating_mul(layers as u64);
        budget.take(usize::try_from(pixels.div_ceil(64)).unwrap_or(usize::MAX))
    }))
}
