use connect::app::App;
use connect::loco_rs::{self, cli};
use migration::Migrator;

fn install_rustls_provider() {
    // rustls 0.23 needs an explicit provider when both aws-lc-rs and ring
    // features are present somewhere in the dependency graph.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> loco_rs::Result<()> {
    install_rustls_provider();
    cli::main::<App, Migrator>().await
}
