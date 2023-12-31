use anyhow::Result;
use fedimint_ln_client::LightningClientModule;
use itertools::Itertools;
use tracing::{error, info};

mod config;
mod error;
mod model;
mod router;
mod state;

mod utils;
use state::{load_fedimint_client, AppState};

use crate::model::app_user_relays::AppUserRelaysBmc;
use crate::model::invoice::InvoiceBmc;
use crate::router::handlers::lnurlp::callback::spawn_invoice_subscription;
use crate::{config::CONFIG, model::ModelManager, state::load_nostr_client};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let state = AppState {
        fm: load_fedimint_client().await?,
        mm: ModelManager::new().await?,
        nostr: load_nostr_client().await?,
    };

    let app = router::create_router(state.clone()).await?;

    // spawn a task to check for previous pending invoices
    tokio::spawn(async move {
        if let Err(e) = handle_pending_invoices(state).await {
            error!("Error handling pending invoices: {e}")
        }
    });

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", CONFIG.domain, CONFIG.port))
        .await
        .unwrap();
    info!("Listening on {}", CONFIG.port);
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

/// Starts subscription for all pending invoices from previous run
async fn handle_pending_invoices(state: AppState) -> Result<()> {
    let invoices = InvoiceBmc::get_pending(&state.mm).await?;
    let ln = state.fm.get_first_module::<LightningClientModule>();

    // sort invoices by user for efficiency
    let invoices_by_user = invoices
        .into_iter()
        .sorted_by_key(|i| i.app_user_id)
        .group_by(|i| i.app_user_id)
        .into_iter()
        .map(|(user, invs)| (user, invs.collect::<Vec<_>>()))
        .collect::<Vec<_>>();

    for (user, invoices) in invoices_by_user {
        let nip05relays = AppUserRelaysBmc::get_by_id(&state.mm, user).await?;
        for invoice in invoices {
            // create subscription to operation if it exists
            if let Ok(subscription) = ln
                .subscribe_ln_receive(invoice.op_id.parse().expect("invalid op_id"))
                .await
            {
                spawn_invoice_subscription(
                    state.clone(),
                    invoice.id,
                    nip05relays.clone(),
                    subscription,
                )
                .await;
            }
        }
    }

    Ok(())
}
