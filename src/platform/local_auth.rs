//! macOS `Approve`: device-owner authentication availability. `canEvaluatePolicy:error:`
//! only queries; `evaluatePolicy` is never called here.

use super::{Approve, Availability};
use objc2_local_authentication::{LAContext, LAPolicy};

pub struct LocalAuth;

impl Approve for LocalAuth {
    fn owner_auth_available(&self) -> Availability {
        // SAFETY: the context is created and queried on this thread only, and the query
        // presents no UI.
        unsafe {
            let context = LAContext::new();
            match context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) {
                Ok(()) => Availability::Available,
                Err(e) => Availability::Unavailable(format!(
                    "{} (LAError {})",
                    e.localizedDescription(),
                    e.code()
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn availability_is_answered_without_waiting_for_a_person() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(LocalAuth.owner_auth_available());
        });
        let answer = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the availability query blocked; it must only query");
        if let Availability::Unavailable(reason) = answer {
            assert!(!reason.is_empty());
        }
    }
}
