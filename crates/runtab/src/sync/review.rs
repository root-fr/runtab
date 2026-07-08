use std::io::Write;

use crate::ledger::{Ledger, ReviewItem};

/// The verbatim consent copy. Kept byte-for-byte identical to the dashboard's
/// `PRIVACY_SENTENCE` (ui/src/lib/privacy.ts) so the promise a user reads is the
/// same everywhere they are about to upload (spec guardrail).
pub const PRIVACY_SENTENCE: &str =
    "Only these derived numbers ever leave your machine. No prompts, no code, no file paths, ever.";

/// The pre-sync review — the CLI's consent moment. Lists the project labels that
/// would sync (basenames, never full paths), prints the privacy sentence, and
/// lets the user exclude any before the first push. Empty input / EOF accepts the
/// defaults, which is itself the consent. Persisting marks the machine reviewed
/// so `pending_batch` will start sending.
pub fn run(ledger: &Ledger) -> anyhow::Result<()> {
    let items = ledger.project_review_items()?;
    if items.is_empty() {
        ledger.set_project_review(&[])?;
        return Ok(());
    }
    println!("\nBefore the first sync, review what will be uploaded.");
    println!("{PRIVACY_SENTENCE}\n");
    println!("Projects that will sync (label = folder name; full paths stay local):");
    for item in &items {
        println!("  - {}", item.name);
    }
    print!(
        "\nPress Enter to sync all {} as-is, or type comma-separated labels to EXCLUDE: ",
        items.len()
    );
    std::io::stdout().flush()?;

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let excluded: Vec<String> = line
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let decided: Vec<ReviewItem> = items
        .into_iter()
        .map(|mut item| {
            item.excluded = excluded.iter().any(|e| e.eq_ignore_ascii_case(&item.name));
            item
        })
        .collect();
    let kept = decided.iter().filter(|i| !i.excluded).count();
    ledger.set_project_review(&decided)?;
    println!(
        "Review saved: {kept} project(s) will sync, {} excluded.",
        decided.len() - kept
    );
    Ok(())
}
