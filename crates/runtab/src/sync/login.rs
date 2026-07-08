use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::client::{PollOutcome, SyncClient, SyncError};
use super::token::TokenStore;
use crate::encoding::new_device_code;
use crate::ledger::Ledger;
use crate::serve::browser;

/// Drive the browser device-authorization flow: mint a `device_code`, print the
/// verification URL + display code, best-effort open the browser, then poll
/// until a signed-in browser approves the request.
pub async fn run(
    ledger: &Mutex<Ledger>,
    client: &SyncClient,
    machine_name: &str,
    server_url: &str,
) -> anyhow::Result<()> {
    let device_code = new_device_code()?;
    let start = client.device_start(machine_name, &device_code).await?;

    println!("To authorize this machine, open:");
    println!("  {}", start.verification_uri_complete);
    println!("Confirmation code (must match the page): {}", start.display_code);
    println!("Waiting for approval… (Ctrl-C to cancel)");
    browser::open(&start.verification_uri_complete);

    let interval = Duration::from_secs(start.interval_s.max(1));
    let deadline = Instant::now() + Duration::from_secs(start.expires_in_s);

    loop {
        if Instant::now() >= deadline {
            anyhow::bail!("the authorization request expired; run `runtab sync login` again");
        }
        tokio::time::sleep(interval).await;
        match client.auth_poll(&device_code).await {
            Ok(PollOutcome::Confirmed { token, user_id, email }) => {
                TokenStore::store(&email, &token)?;
                let l = lock(ledger)?;
                l.enable_sync(&email, &user_id, server_url)?;
                println!(
                    "Device authorized. Sync is on for {email}. If that isn't your account, run \
                     `runtab sync off` now."
                );
                return Ok(());
            }
            Ok(PollOutcome::Pending) => continue,
            Ok(PollOutcome::Expired) => {
                anyhow::bail!("the authorization request expired; run `runtab sync login` again");
            }
            Ok(PollOutcome::MachineLimit) => {
                anyhow::bail!(
                    "machine limit reached — revoke a machine at runtab.ai/account, then re-run \
                     `runtab sync login`"
                );
            }
            // A transient network blip must not abort the wait.
            Err(SyncError::Network(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

fn lock(ledger: &Mutex<Ledger>) -> anyhow::Result<std::sync::MutexGuard<'_, Ledger>> {
    ledger
        .lock()
        .map_err(|_| anyhow::anyhow!("ledger lock poisoned"))
}
