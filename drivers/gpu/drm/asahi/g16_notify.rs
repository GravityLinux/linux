// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! RTKit and scheduler wakeups for the single G16 submission worker.

use kernel::sync::{CondVar, CondVarTimeoutResult, Mutex};
use kernel::{prelude::*, time::msecs_to_jiffies};

#[pin_data]
pub(crate) struct Notifications {
    #[pin]
    state: Mutex<State>,
    #[pin]
    changed: CondVar,
}

struct State {
    generation: u64,
    crashed: bool,
}

impl Notifications {
    pub(crate) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            state <- kernel::new_mutex!(State { generation: 0, crashed: false }),
            changed <- kernel::new_condvar!(),
        })
    }

    /// Called from RTKit's threaded callback or a DRM scheduler callback.
    /// Neither callback needs the submission engine or firmware memory lock.
    pub(crate) fn notify(&self, crashed: bool) {
        let mut state = self.state.lock();
        state.crashed |= crashed;
        state.generation = state.generation.wrapping_add(1);
        self.changed.notify_one();
    }

    /// Sample before inspecting firmware or ready jobs. A notification between
    /// that inspection and sleeping must cause another inspection, not be lost.
    pub(crate) fn snapshot(&self) -> Result<u64> {
        let state = self.state.lock();
        if state.crashed {
            return Err(EIO);
        }
        Ok(state.generation)
    }

    /// Sleep until the sampled generation changes or the progress watchdog
    /// expires. Spurious wakeups preserve the remaining timeout.
    pub(crate) fn wait(&self, generation: u64, timeout_ms: u32) -> Result {
        let mut remaining = msecs_to_jiffies(timeout_ms.max(1));
        let mut state = self.state.lock();
        while state.generation == generation && !state.crashed {
            remaining = match self
                .changed
                .wait_interruptible_timeout(&mut state, remaining)
            {
                CondVarTimeoutResult::Woken { jiffies }
                | CondVarTimeoutResult::Signal { jiffies } => jiffies,
                CondVarTimeoutResult::Timeout => {
                    if state.generation == generation && !state.crashed {
                        return Err(ETIMEDOUT);
                    }
                    break;
                }
            };
        }
        if state.crashed {
            return Err(EIO);
        }
        Ok(())
    }
}
