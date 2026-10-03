use nasiko_llm_router::routing::switch_cost::should_switch;
use nasiko_pricing::PricingEngine;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var("DATABASE_URL")?)
        .await?;
    for table in ["flows", "token_usage", "model_pricing"] {
        sqlx::query(&format!(
            "CREATE TEMP TABLE {table} (LIKE public.{table} INCLUDING DEFAULTS)"
        ))
        .execute(&db)
        .await?;
    }
    let user = "00000000-0000-0000-0000-000000000001";
    let agent = "00000000-0000-0000-0000-000000000002";
    sqlx::query("INSERT INTO flows (flow_id,user_id,metadata) VALUES ('demo-flow',$1::uuid,'{\"context_id\":\"demo-conversation\"}')")
        .bind(user).execute(&db).await?;
    sqlx::query("INSERT INTO model_pricing (provider,model,input_price_per_1m,output_price_per_1m,cache_read_price_per_1m,cache_creation_price_per_1m) VALUES ('demo','current',10,20,1,10),('demo','candidate',5,10,0.5,5)")
        .execute(&db).await?;
    sqlx::query("INSERT INTO token_usage (user_id,agent_id,operation_type,provider,model,session_id,input_tokens,output_tokens,cache_read_input_tokens,cache_creation_input_tokens,finish_reason,metadata) VALUES ($1::uuid,$2::uuid,'direct_llm','demo','current','demo-flow',100,100,10000,0,'stop','{\"routing_evidence\":{\"input_reported\":true,\"output_reported\":true,\"cache_read_reported\":true,\"cache_creation_reported\":true}}')")
        .bind(user).bind(agent).execute(&db).await?;
    let pricing = PricingEngine::new(db.clone());
    let retained = should_switch(
        &db,
        &pricing,
        user,
        agent,
        "demo-conversation",
        "demo",
        "current",
        "candidate",
        0.2,
    )
    .await;
    assert!(
        !retained,
        "Cached current model must stay despite cheaper candidate list price"
    );
    println!("Cached case: KEEP. Estimated stay $0.013; switch $0.0515.");
    sqlx::query("UPDATE token_usage SET input_tokens=10100,cache_read_input_tokens=0")
        .execute(&db)
        .await?;
    let switched = should_switch(
        &db,
        &pricing,
        user,
        agent,
        "demo-conversation",
        "demo",
        "current",
        "candidate",
        0.2,
    )
    .await;
    assert!(
        switched,
        "Uncached candidate saves more than the required margin"
    );
    println!("Uncached case: SWITCH. Estimated stay $0.103; switch $0.0515.");
    sqlx::query("UPDATE token_usage SET metadata='{}'")
        .execute(&db)
        .await?;
    let unknown = should_switch(
        &db,
        &pricing,
        user,
        agent,
        "demo-conversation",
        "demo",
        "current",
        "candidate",
        0.2,
    )
    .await;
    assert!(!unknown, "Missing cache evidence must retain current model");
    println!("Missing evidence: KEEP.");
    println!(
        "Synthetic usage and rates in PostgreSQL temporary tables. No answer-model calls. Temporary tables disappear on exit."
    );
    db.close().await;
    Ok(())
}
