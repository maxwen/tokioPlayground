use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use parking_lot::{Condvar, Mutex};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

#[derive(Default, Debug)]
struct Waiter {
    position_reached: bool,
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

    fn wait_for_position_reached(&self) {
        let (mutex, cvar) = self.0.as_ref();
        let mut waiter = mutex.lock();

        cvar.wait_while(&mut waiter, |waiter| {
            !waiter.position_reached
        });
    }
}