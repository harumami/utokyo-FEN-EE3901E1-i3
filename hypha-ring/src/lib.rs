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
        let mut head = *self.head.get_mut();
        let tail = *self.tail.get_mut();

        while head != tail {
            unsafe { (&mut *self.values[head].get()).assume_init_drop() };
            head = (head + 1) % N;
        }
    }
}

unsafe impl<T, const N: usize> Send for Inner<T, N> {}
unsafe impl<T, const N: usize> Sync for Inner<T, N> {}

#[cfg(test)]
mod tests {
    use {
        super::Ring,
        ::std::{
            sync::atomic::{
                AtomicUsize,
                Ordering,
            },
            thread::{
                spawn as spawn_thread,
                yield_now,
            },
            time::Duration,
        },
        ::tokio::{
            task::spawn as spawn_task,
            time::sleep,
        },
    };

    #[test]
    fn simple_push_and_pop() {
        let ring = Ring::<i32, 4>::new();
        let consumer = ring.consumer().unwrap();
        let producer = ring.producer().unwrap();
        assert!(ring.is_empty());
        producer.push(10).unwrap();
        assert!(!ring.is_empty());
        assert_eq!(consumer.pop(), Some(10));
        assert!(ring.is_empty());
    }

    #[test]
    fn full_and_empty_logic() {
        let ring = Ring::<i32, 4>::new();
        let consumer = ring.consumer().unwrap();
        let producer = ring.producer().unwrap();
        assert_eq!(ring.len(), 0);
        producer.push(1).unwrap();
        producer.push(2).unwrap();
        producer.push(3).unwrap();
        assert_eq!(ring.len(), 3);
        assert!(producer.push(4).is_err());
        assert_eq!(consumer.pop(), Some(1));
        assert_eq!(ring.len(), 2);
        producer.push(4).unwrap();
        assert_eq!(ring.len(), 3);
        assert_eq!(consumer.pop(), Some(2));
        assert_eq!(consumer.pop(), Some(3));
        assert_eq!(consumer.pop(), Some(4));
        assert_eq!(ring.len(), 0);
        assert!(ring.is_empty());
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn wrap_around_behavior() {
        let ring = Ring::<i32, 4>::new();
        let consumer = ring.consumer().unwrap();
        let producer = ring.producer().unwrap();
        producer.push(1).unwrap();
        producer.push(2).unwrap();
        assert_eq!(consumer.pop(), Some(1));
        assert_eq!(consumer.pop(), Some(2));
        producer.push(3).unwrap();
        producer.push(4).unwrap();
        producer.push(5).unwrap();
        assert_eq!(ring.len(), 3);
        assert_eq!(consumer.pop(), Some(3));
        assert_eq!(consumer.pop(), Some(4));
        assert_eq!(consumer.pop(), Some(5));
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn spsc_threaded_transfer() {
        let ring = Ring::<usize, 1024>::new();
        let consumer = ring.consumer().unwrap();
        let producer = ring.producer().unwrap();
        let iterations = 100_000;

        let producer_thread = spawn_thread(move || {
            for i in 0..iterations {
                while producer.push(i).is_err() {
                    yield_now();
                }
            }
        });

        let consumer_thread = spawn_thread(move || {
            for i in 0..iterations {
                loop {
                    if let Some(value) = consumer.pop() {
                        assert_eq!(value, i);
                        break;
                    }

                    yield_now();
                }
            }
        });

        producer_thread.join().unwrap();
        consumer_thread.join().unwrap();
    }

    #[tokio::test]
    async fn async_notified() {
        let ring = Ring::<i32, 4>::new();

        let consumer_handle = spawn_task({
            let ring = ring.clone();

            async move {
                let mut consumer = ring.consumer().unwrap();
                consumer.notified().await;
                assert_eq!(consumer.pop(), Some(123));
            }
        });

        let producer_handle = spawn_task({
            let ring = ring.clone();

            async move {
                let producer = ring.producer().unwrap();
                sleep(Duration::from_millis(10)).await;
                producer.push(123).unwrap();
            }
        });

        producer_handle.await.unwrap();
        consumer_handle.await.unwrap();
    }

    #[test]
    fn drop_cleans_up_items() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        struct DropTracker;

        impl Drop for DropTracker {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        DROP_COUNT.store(0, Ordering::SeqCst);

        {
            let ring = Ring::<DropTracker, 4>::new();
            let _consumer = ring.consumer().unwrap();
            let producer = ring.producer().unwrap();
            producer.push(DropTracker).map_err(|_| ()).unwrap();
            producer.push(DropTracker).map_err(|_| ()).unwrap();
        }

        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 2);
    }
}
