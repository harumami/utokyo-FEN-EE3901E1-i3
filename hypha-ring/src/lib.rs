use {
    ::futures::task::AtomicWaker,
    ::std::{
        cell::UnsafeCell,
        mem::MaybeUninit,
        sync::{
            Arc,
            atomic::{
                AtomicBool,
                AtomicUsize,
                Ordering,
            },
        },
    },
};

#[derive(Clone)]
pub struct Ring<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Ring<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner::new(capacity)),
        }
    }
}

pub struct Producer<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    producer: AtomicBool,
    consumer: AtomicBool,
    waker: AtomicWaker,
    head: AtomicUsize,
    tail: AtomicUsize,
    values: UnsafeCell<Box<[MaybeUninit<T>]>>,
}

impl<T> Inner<T> {
    fn new(capacity: usize) -> Self {
        Self {
            producer: AtomicBool::new(false),
            consumer: AtomicBool::new(false),
            waker: AtomicWaker::new(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            values: UnsafeCell::new(Box::new_uninit_slice(capacity)),
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let values = self.values.get_mut();

        for i in head..tail {
            unsafe { values[i].assume_init_drop() };
        }
    }
}
