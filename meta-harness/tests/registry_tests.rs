use std::sync::Arc;
use harness_protocol::arch::{
    ComponentFilter, ComponentSpec, GraphPerspective, SubtreeQuery, SystemArchitecture,
    TaskSpec,
};
use meta_harness::registry::{ArchitectureRepository, FsJsonRepository, RegistryService};

#[tokio::test]
async fn test_fs_repo_crud_and_atomic_save() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let json_path = tmp_dir.path().join("test_architecture.json");

    let repo = FsJsonRepository::new_or_init(&json_path).await.unwrap();

    // 1. Initially empty
    let full = repo.load_full().await.unwrap();
    assert!(full.components.is_empty());

    // 2. Save a component
    let comp = ComponentSpec {
        id: "auth::TokenValidator".to_string(),
        name: "Token Validator".to_string(),
        comp_type: "class".to_string(),
        description: "Validates JWT tokens".to_string(),
        status: "new".to_string(),
        stage: "declared".to_string(),
        parent_id: None,
        implements_id: None,
        location: None,
        inputs: None,
        outputs: None,
        properties: None,
        side_effects: vec![],
        implementation_spec: None,
        modification_tasks: vec![],
    };

    repo.save_component(&comp).await.unwrap();

    // 3. Retrieve by ID
    let fetched = repo.get_component("auth::TokenValidator").await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().name, "Token Validator");

    // 4. Query with filter
    let filter = ComponentFilter {
        comp_type: Some("class".to_string()),
        ..Default::default()
    };
    let query_res = repo.query_components(&filter).await.unwrap();
    assert_eq!(query_res.len(), 1);
    assert_eq!(query_res[0].id, "auth::TokenValidator");

    // 5. Verify physical disk file exists and is valid JSON
    assert!(json_path.exists());
    let raw_content = tokio::fs::read_to_string(&json_path).await.unwrap();
    let disk_arch: SystemArchitecture = serde_json::from_str(&raw_content).unwrap();
    assert!(disk_arch.components.contains_key("auth::TokenValidator"));

    // 6. Delete
    repo.delete_component("auth::TokenValidator").await.unwrap();
    assert!(repo.get_component("auth::TokenValidator").await.unwrap().is_none());
}

#[tokio::test]
async fn test_registry_service_inspection_and_discovery() {
    let json_raw = include_str!("../../system_architecture.json");
    let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();

    let tmp_dir = tempfile::tempdir().unwrap();
    let json_path = tmp_dir.path().join("live_architecture.json");

    let repo = Arc::new(FsJsonRepository::from_architecture(arch, json_path));
    let service = RegistryService::new(repo);

    // 1. find_components by query text
    let filter_text = ComponentFilter {
        query: Some("calculator".to_string()),
        ..Default::default()
    };
    let found = service.find_components(&filter_text).await.unwrap();
    assert!(!found.is_empty());
    assert!(found.iter().all(|c| c.id.contains("calculator") || c.name.to_lowercase().contains("calculator")));

    // 2. inspect_component
    let spec = service.inspect_component("calculator_service_calculate").await.unwrap();
    assert_eq!(spec.name, "Calculate Method");
    assert_eq!(spec.comp_type, "function");
    assert_eq!(spec.stage, "implemented");

    // 3. get_summary
    let summary = service.get_summary().await.unwrap();
    assert!(summary.total_components > 0);
    assert!(summary.stages_count.contains_key("implemented"));
    assert!(summary.workflow_trees.contains(&"calculation_flow".to_string()));
}

#[tokio::test]
async fn test_registry_service_subtree_slicing_dual_projection() {
    let json_raw = include_str!("../../system_architecture.json");
    let arch: SystemArchitecture = serde_json::from_str(json_raw).unwrap();

    let tmp_dir = tempfile::tempdir().unwrap();
    let json_path = tmp_dir.path().join("arch_slice.json");

    let repo = Arc::new(FsJsonRepository::from_architecture(arch, json_path));
    let service = RegistryService::new(repo);

    // Slicing Call Flow at depth 1
    let query_depth1 = SubtreeQuery {
        root_id: "calculator_client_run".to_string(),
        perspective: GraphPerspective::CallFlow,
        depth: 1,
    };
    let (sub_graph1, text_tree1) = service.get_subtree(&query_depth1).await.unwrap();
    assert_eq!(sub_graph1.nodes.len(), 2);
    assert!(text_tree1.contains("calculator_client_run"));
    assert!(text_tree1.contains("calculator_service_calculate"));

    // Slicing Call Flow at depth 2 (includes indirect dependencies)
    let query_depth2 = SubtreeQuery {
        root_id: "calculator_client_run".to_string(),
        perspective: GraphPerspective::CallFlow,
        depth: 2,
    };
    let (sub_graph2, text_tree2) = service.get_subtree(&query_depth2).await.unwrap();
    assert_eq!(sub_graph2.nodes.len(), 4);
    assert!(text_tree2.contains("math_validator_validate"));
    assert!(text_tree2.contains("audit_logger_log"));
}

#[tokio::test]
async fn test_registry_service_lifecycle_invariants() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let json_path = tmp_dir.path().join("lifecycle.json");

    let repo = Arc::new(FsJsonRepository::new_or_init(&json_path).await.unwrap());
    let service = RegistryService::new(repo);

    // 1. Declare component
    let comp = ComponentSpec {
        id: "payment::StripeGateway".to_string(),
        name: "Stripe Gateway".to_string(),
        comp_type: "class".to_string(),
        description: "Handles payment intents".to_string(),
        status: "new".to_string(),
        stage: "declared".to_string(),
        parent_id: None,
        implements_id: None,
        location: None,
        inputs: None,
        outputs: None,
        properties: None,
        side_effects: vec![],
        implementation_spec: None,
        modification_tasks: vec![],
    };

    let declared = service.declare_component(comp).await.unwrap();
    assert_eq!(declared.stage, "declared");

    // 2. Direct implement or plan from declared stage must fail invariants!
    let err_impl = service.implement_component("payment::StripeGateway").await;
    assert!(err_impl.is_err(), "Cannot implement component in declared stage");

    // 3. Approve architecture
    let approved = service.approve_architecture("payment::StripeGateway").await.unwrap();
    assert_eq!(approved.stage, "arch_approved");

    // Still cannot implement directly without plan approval!
    let err_impl2 = service.implement_component("payment::StripeGateway").await;
    assert!(err_impl2.is_err(), "Cannot implement without approved plan");

    // 4. Plan component
    let plan_spec = serde_json::json!({
        "strategy": "Wrap official stripe-rust SDK",
        "retry_policy": "exponential"
    });
    let tasks = vec![TaskSpec {
        id: Some("task_1".to_string()),
        task: "Implement charge method".to_string(),
        completed: false,
        subtasks: vec![],
    }];

    let planned = service
        .plan_component("payment::StripeGateway", plan_spec, tasks)
        .await
        .unwrap();
    assert_eq!(planned.stage, "plan_approved");
    assert_eq!(planned.modification_tasks.len(), 1);

    // 5. Implement component
    let implemented = service.implement_component("payment::StripeGateway").await.unwrap();
    assert_eq!(implemented.stage, "implemented");
}
