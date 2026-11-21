//! The bridge between GPUI's executor and fastdash-core's async code.
//!
//! On the desktop, connectors use reqwest, which needs a tokio runtime; GPUI
//! drives its own executor on the platform event loop. One small multi-threaded
//! tokio runtime runs every fetch and keychain call off the UI thread, and
//! GPUI tasks simply await the returned future.
//!
//! In the browser there is no tokio runtime. The same functions poll the
//! future on GPUI's executor (reqwest uses `fetch`) and run blocking closures
//! inline. There is no filesystem or keychain work to move off the UI thread.

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::future::Future;
    use std::sync::OnceLock;

    use tokio::runtime::Runtime;

    fn runtime() -> &'static Runtime {
        static RT: OnceLock<Runtime> = OnceLock::new();
        RT.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("fastdash-io")
                .enable_all()
                .build()
                .expect("failed to start the tokio runtime")
        })
    }

    /// Run `fut` on the tokio runtime. The returned future resolves to its output,
    /// or `None` if the task panicked or was cancelled.
    pub fn spawn<F>(fut: F) -> impl Future<Output = Option<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let handle = runtime().spawn(fut);
        async move { handle.await.ok() }
    }

    /// Run `fut` on the tokio runtime and forget it: nothing waits on the result.
    pub fn detach<F>(fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        runtime().spawn(fut);
    }

    /// Like [`spawn`], but the tokio task is aborted when the returned future is
    /// dropped - for work the user can cancel, such as a sign-in that polls.
    pub fn spawn_abortable<F>(fut: F) -> impl Future<Output = Option<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
        impl<T> Drop for AbortOnDrop<T> {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let mut guard = AbortOnDrop(runtime().spawn(fut));
        async move { (&mut guard.0).await.ok() }
    }

    /// Run blocking work (keychain, filesystem) on tokio's blocking pool.
    pub fn spawn_blocking<R: Send + 'static>(
        f: impl FnOnce() -> R + Send + 'static,
    ) -> impl Future<Output = Option<R>> {
        let handle = runtime().spawn_blocking(f);
        async move { handle.await.ok() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::*;

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::future::Future;

    pub fn spawn<F>(fut: F) -> impl Future<Output = Option<F::Output>>
    where
        F: Future + 'static,
    {
        async move { Some(fut.await) }
    }

    pub fn detach<F>(fut: F)
    where
        F: Future<Output = ()> + 'static,
    {
        wasm_bindgen_futures::spawn_local(fut);
    }

    pub fn spawn_abortable<F>(fut: F) -> impl Future<Output = Option<F::Output>>
    where
        F: Future + 'static,
    {
        let (handle, registration) = futures::future::AbortHandle::new_pair();
        struct Guard(futures::future::AbortHandle);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let guard = Guard(handle);
        let abortable = futures::future::Abortable::new(fut, registration);
        async move {
            let _guard = guard;
            abortable.await.ok()
        }
    }

    pub fn spawn_blocking<R: 'static>(
        f: impl FnOnce() -> R + 'static,
    ) -> impl Future<Output = Option<R>> {
        let value = f();
        async move { Some(value) }
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::*;
