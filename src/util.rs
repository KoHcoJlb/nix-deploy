use std::error::Error;

use eyre::Report;

#[allow(clippy::disallowed_methods)]
pub fn downcast_ref<E: Error + Send + Sync + 'static>(err: &Report) -> Option<&E> {
    if let Some(err) = err.downcast_ref::<E>() {
        Some(err)
    } else {
        err.chain().find_map(|e| e.downcast_ref::<E>())
    }
}

pub fn is<E: Error + Send + Sync + 'static>(err: &Report) -> bool {
    downcast_ref::<E>(err).is_some()
}
