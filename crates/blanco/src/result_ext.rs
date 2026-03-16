pub trait ResultExt<T> {
    fn log_err(self) -> Option<T>;
}

impl<T> ResultExt<T> for anyhow::Result<T> {
    #[track_caller]
    fn log_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                let caller = std::panic::Location::caller();
                tracing::error!("{}:{}: {:#}", caller.file(), caller.line(), error);
                None
            }
        }
    }
}
