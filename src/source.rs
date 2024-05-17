use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use parking_lot::{Condvar, Mutex};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

#[derive(Default, Debug)]
struct Waiter {
    position_reached: bool,
    stream_done: bool,
}


#[derive(Debug, Clone)]
pub(crate) struct SourceHandle {
    pub(crate) position_reached: PositionReached,
}

impl SourceHandle {

    pub fn wait_for_requested_position(&self) {
        self.position_reached.wait_for_position_reached();
    }
}

#[derive(Default, Clone, Debug)]
pub(crate) struct PositionReached(Arc<(Mutex<Waiter>, Condvar)>);

impl PositionReached {
    pub(crate) fn notify_position_reached(&self) {
        let (mutex, cvar) = self.0.as_ref();
        mutex.lock().position_reached = true;
        cvar.notify_all();
    }

    fn notify_stream_done(&self) {
        let (mutex, cvar) = self.0.as_ref();
        mutex.lock().stream_done = true;
        cvar.notify_all();
    }

    fn wait_for_position_reached(&self) {
        let (mutex, cvar) = self.0.as_ref();
        let mut waiter = mutex.lock();
        if waiter.stream_done {
            return;
        }


        cvar.wait_while(&mut waiter, |waiter| {
            !waiter.stream_done && !waiter.position_reached
        });

        if !waiter.stream_done {
            waiter.position_reached = false;
        }
    }
}