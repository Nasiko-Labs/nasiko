//! Gherkin runner. Scenarios live in `tests/features`. Fixtures live in `tests/fixtures`.

#[path = "common/sample.rs"]
mod sample;
#[path = "behavior/world.rs"]
mod world;
#[path = "behavior/steps.rs"]
mod steps;

use cucumber::World as _;
use world::CompactWorld;

#[tokio::main]
async fn main() {
    let features = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/features");
    CompactWorld::run(features).await;
}
