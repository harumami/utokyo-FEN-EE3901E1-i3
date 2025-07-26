use {
    ::crossbeam::utils::CachePadded,
    ::futures::{
        future::poll_fn,
        task::AtomicWaker,
    },
    ::std::{
        cell::{
            Cell,
            UnsafeCell,
        },
        iter::{
            from_fn,
            once,
        },
        mem::MaybeUninit,
        sync::{
            Arc,
            atomic::{
                AtomicBool,
                AtomicUsize,
                Ordering,
            },
        },
        task::Poll,
    },
};

#[derive(Clone, Debug)]
pub struct Ring<T, const N: usize> {
    inner: Arc<Inner<T, N>>,
}

impl<T, const N: usize> Ring<T, N> {
    pub fn new() -> Self {
        const {
            if N == 0 {
                panic!("N must be not 0");
            }
        }

        Self {
            inner: Arc::new(Inner {
                producer: CachePadded::new(AtomicBool::new(false)),
                consumer: CachePadded::new(AtomicBool::new(false)),
                head: CachePadded::new(AtomicUsize::new(0)),
                tail: CachePadded::new(AtomicUsize::new(0)),
                waker: CachePadded::new(AtomicWaker::new()),
                values: unsafe { MaybeUninit::<[_; N]>::uninit().assume_init() }
                    .map(UnsafeCell::new),
            }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.head.load(Ordering::Acquire) == self.inner.tail.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        (N + self.inner.tail.load(Ordering::Acquire) - self.inner.head.load(Ordering::Acquire)) % N
    }

    pub fn consumer(&self) -> Option<Consumer<T, N>> {
        match self
            .inner
            .consumer
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        {
            Result::Ok(_) => Option::Some(Consumer {
                head: Cell::new(self.inner.head.load(Ordering::Acquire)),
                inner: self.inner.clone(),
            }),
            Result::Err(_) => Option::None,
        }
    }

    pub fn producer(&self) -> Option<Producer<T, N>> {
        match self
            .inner
            .producer
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        {
            Result::Ok(_) => Option::Some(Producer {
                tail: Cell::new(self.inner.tail.load(Ordering::Acquire)),
                inner: self.inner.clone(),
            }),
            Result::Err(_) => Option::None,
        }
    }
}

impl<T, const N: usize> Default for Ring<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Consumer<T, const N: usize> {
    head: Cell<usize>,
    inner: Arc<Inner<T, N>>,
}

impl<T, const N: usize> Consumer<T, N> {
    pub fn is_empty(&self) -> bool {
        self.head.get() == self.inner.tail.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        (N + self.inner.tail.load(Ordering::Acquire) - self.head.get()) % N
    }

    pub fn pop(&self) -> Option<T> {
        let head = self.head.get();

        if head == self.inner.tail.load(Ordering::Acquire) {
            return Option::None;
        }

        let value = unsafe { (&*self.inner.values[head].get()).assume_init_read() };
        let next_head = (head + 1) % N;
        self.head.set(next_head);
        self.inner.head.store(next_head, Ordering::Release);
        Option::Some(value)
    }

    pub fn drain(&self) -> impl Iterator<Item = T> {
        from_fn(|| self.pop())
    }

    pub fn notified(&mut self) -> impl Send + Future<Output = ()> {
        let head = self.head.get();

        poll_fn(move |context| {
            if head != self.inner.tail.load(Ordering::Acquire) {
                return Poll::Ready(());
            }

            self.inner.waker.register(context.waker());

            if head != self.inner.tail.load(Ordering::Acquire) {
                return Poll::Ready(());
            }

            Poll::Pending
        })
    }
}

impl<T, const N: usize> Drop for Consumer<T, N> {
    fn drop(&mut self) {
        self.inner.consumer.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
pub struct Producer<T, const N: usize> {
    tail: Cell<usize>,
    inner: Arc<Inner<T, N>>,
}

impl<T, const N: usize> Producer<T, N> {
    pub fn is_empty(&self) -> bool {
        self.inner.head.load(Ordering::Acquire) == self.tail.get()
    }

    pub fn len(&self) -> usize {
        (N + self.tail.get() - self.inner.head.load(Ordering::Acquire)) % N
    }

    pub fn push(&self, value: T) -> Result<(), T> {
        let tail = self.tail.get();
        let next_tail = (tail + 1) % N;

        if next_tail == self.inner.head.load(Ordering::Acquire) {
            return Result::Err(value);
        }

        unsafe { &mut *self.inner.values[tail].get() }.write(value);
        self.tail.set(next_tail);
        self.inner.tail.store(next_tail, Ordering::Release);
        self.inner.waker.wake();
        Result::Ok(())
    }

    pub fn extend(
        &self,
        mut values: impl Iterator<Item = T>,
    ) -> Result<(), impl Iterator<Item = T>> {
        while let Option::Some(value) = values.next() {
            if let Result::Err(value) = self.push(value) {
                return Result::Err(once(value).chain(values));
            }
        }

        Result::Ok(())
    }
}

impl<T, const N: usize> Drop for Producer<T, N> {
    fn drop(&mut self) {
        self.inner.producer.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
struct Inner<T, const N: usize> {
    producer: CachePadded<AtomicBool>,
    consumer: CachePadded<AtomicBool>,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    waker: CachePadded<AtomicWaker>,
    values: [UnsafeCell<MaybeUninit<T>>; N],
}

impl<T, const N: usize> Drop for Inner<T, N> {
    fn drop(&mut self) {
        for i in self.head.load(Ordering::Acquire)..self.tail.load(Ordering::Acquire) {
            unsafe { (&mut *self.values[i % N].get()).assume_init_drop() };
        }
    }
}

unsafe impl<T, const N: usize> Send for Inner<T, N> {}
unsafe impl<T, const N: usize> Sync for Inner<T, N> {}
