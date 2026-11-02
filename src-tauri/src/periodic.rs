use std::future::Future;
use std::time::Duration;

/// Each caller owns a separate loop; a slow inbox tick cannot delay scheduling.
pub(crate) async fn run_periodic<F, Tick>(period: Duration, mut tick: F)
where
    F: FnMut() -> Tick,
    Tick: Future<Output = ()>,
{
    loop {
        tokio::time::sleep(period).await;
        tick().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn stalled_inbox_does_not_block_scheduler() {
        let (inbox_started, inbox_entered) = oneshot::channel();
        let mut inbox_started = Some(inbox_started);
        let inbox = tokio::spawn(run_periodic(Duration::from_millis(1), move || {
            if let Some(started) = inbox_started.take() {
                let _ = started.send(());
            }
            std::future::pending::<()>()
        }));
        tokio::time::timeout(Duration::from_secs(2), inbox_entered)
            .await
            .unwrap()
            .unwrap();

        let (scheduled, scheduler_ran) = oneshot::channel();
        let mut scheduled = Some(scheduled);
        let scheduler = tokio::spawn(run_periodic(Duration::from_millis(1), move || {
            if let Some(scheduled) = scheduled.take() {
                let _ = scheduled.send(());
            }
            std::future::ready(())
        }));
        let result = tokio::time::timeout(Duration::from_secs(2), scheduler_ran).await;
        inbox.abort();
        scheduler.abort();
        assert_eq!(result.unwrap().unwrap(), ());
    }
}
