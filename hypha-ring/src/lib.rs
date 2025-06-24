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

    pub fn producer(&self) -> Option<Producer<T>> {
        Producer::new(self)
    }

    pub fn consumer(&self) -> Option<Consumer<T>> {
        Consumer::new(self)
    }
}

pub struct Producer<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Producer<T> {
    pub fn new(ring: &Ring<T>) -> Option<Self> {
        let inner = &ring.inner;

        match inner.producer.fetch_or(true, Ordering::AcqRel) {
            false => Option::Some(Self {
                inner: inner.clone(),
            }),
            true => Option::None,
        }
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.inner.producer.store(false, Ordering::Release);
    }
}

pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Consumer<T> {
    pub fn new(ring: &Ring<T>) -> Option<Self> {
        let inner = &ring.inner;

        match inner.consumer.fetch_or(true, Ordering::AcqRel) {
            false => Option::Some(Self {
                inner: inner.clone(),
            }),
            true => Option::None,
        }
    }
}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        self.inner.consumer.store(false, Ordering::Release);
    }
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

    unsafe fn push(&self, values: &[T]) {}
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let values = self.values.get_mut();

        for i in head..tail {
            unsafe { values[i % values.len()].assume_init_drop() };
        }
    }
}
