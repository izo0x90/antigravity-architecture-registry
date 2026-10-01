use std::sync::Arc;
use harness_protocol::arch::{
    ArchitectureSummary, ComponentFilter, ComponentSpec, ComponentSummary,
    SubtreeQuery, SystemArchitecture, TaskSpec, UnifiedArchitectureGraph,
};
use crate::registry::error::RegistryError;
use crate::registry::traits::ArchitectureRepository;

/// Domain application service coordinating architecture queries, lifecycle invariants,
/// and dual projections (visual graphs and textual tree plans).
#[derive(Clone)]
pub struct RegistryService {
    repo: Arc<dyn ArchitectureRepository>,
}

impl RegistryService {
    pub fn new(repo: Arc<dyn ArchitectureRepository>) -> Self {
        Self { repo }
    }

    // -------------------------------------------------------------------------
    // 1. Discovery & Inspection (Storage Agnostic)
    // -------------------------------------------------------------------------

    /// Search components across IDs, names, types, stages, and tags.
    /// Returns token-efficient summaries rather than full specifications.
    pub async fn find_components(
        &self,
        filter: &ComponentFilter,
    ) -> Result<Vec<ComponentSummary>, RegistryError> {
        let components = self.repo.query_components(filter).await?;
        Ok(components.iter().map(ComponentSummary::from_spec).collect())
    }

    /// Fetches the complete specification of a single component.
    pub async fn inspect_component(&self, id: &str) -> Result<ComponentSpec, RegistryError> {
        self.repo
            .get_component(id)
            .await?
            .ok_or_else(|| RegistryError::ComponentNotFound(id.to_string()))
    }

    /// Generates high-level architecture statistics and root modules.
    pub async fn get_summary(&self) -> Result<ArchitectureSummary, RegistryError> {
        let full = self.repo.load_full().await?;
        Ok(full.summarize())
    }

    // -------------------------------------------------------------------------
    // 2. Subtree Slicing & Dual Projections (Visual Graph + Textual Tree)
    // -------------------------------------------------------------------------

    /// Extracts an isolated subtree rooted at `query.root_id` with perspective and depth.
    /// Returns:
    /// - `UnifiedArchitectureGraph`: Targeted node/edge slice for the Cockpit UI.
    /// - `String`: Compact indented ASCII tree plan for LLM agent prompt injection.
    pub async fn get_subtree(
        &self,
        query: &SubtreeQuery,
    ) -> Result<(UnifiedArchitectureGraph, String), RegistryError> {
        let full = self.repo.load_full().await?;
        if !full.components.contains_key(&query.root_id) && !full.usage_trees.contains_key(&query.root_id) {
            return Err(RegistryError::ComponentNotFound(query.root_id.clone()));
        }

        let graph = full.to_graph(query.perspective);
        let sub_graph = graph.extract_subtree(&query.root_id, query.depth);
        let textual_tree = full.to_textual_tree(&query.root_id, query.perspective, query.depth);

        Ok((sub_graph, textual_tree))
    }

    // -------------------------------------------------------------------------
    // 3. Declarative Mutations
    // -------------------------------------------------------------------------

    /// Declares a new component in the architecture.
    pub async fn declare_component(
        &self,
        mut comp: ComponentSpec,
    ) -> Result<ComponentSpec, RegistryError> {
        if self.repo.get_component(&comp.id).await?.is_some() {
            return Err(RegistryError::ComponentAlreadyExists(comp.id));
        }

        if comp.stage.is_empty() {
            comp.stage = "declared".to_string();
        }
        if comp.status.is_empty() {
            comp.status = "new".to_string();
        }

        self.repo.save_component(&comp).await?;
        Ok(comp)
    }

    /// Updates an existing component specification.
    pub async fn update_component(
        &self,
        comp: ComponentSpec,
    ) -> Result<ComponentSpec, RegistryError> {
        if self.repo.get_component(&comp.id).await?.is_none() {
            return Err(RegistryError::ComponentNotFound(comp.id));
        }

        self.repo.save_component(&comp).await?;
        Ok(comp)
    }

    /// Deletes a component from the architecture.
    pub async fn delete_component(&self, id: &str) -> Result<(), RegistryError> {
        if self.repo.get_component(id).await?.is_none() {
            return Err(RegistryError::ComponentNotFound(id.to_string()));
        }

        self.repo.delete_component(id).await?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // 4. Lifecycle Invariant State Transitions
    // -------------------------------------------------------------------------

    /// Approves architecture for a declared or draft component:
    /// Invariant: Must be in 'declared' or 'draft' stage.
    pub async fn approve_architecture(&self, id: &str) -> Result<ComponentSpec, RegistryError> {
        let mut comp = self.inspect_component(id).await?;

        if comp.stage != "declared" && comp.stage != "draft" {
            return Err(RegistryError::InvalidTransition(format!(
                "Cannot approve architecture for component '{}' currently in stage '{}'",
                id, comp.stage
            )));
        }

        comp.stage = "arch_approved".to_string();
        self.repo.save_component(&comp).await?;
        Ok(comp)
    }

    /// Submits and approves the implementation plan for a component:
    /// Invariant: Must be in 'arch_approved' stage.
    pub async fn plan_component(
        &self,
        id: &str,
        implementation_spec: serde_json::Value,
        tasks: Vec<TaskSpec>,
    ) -> Result<ComponentSpec, RegistryError> {
        let mut comp = self.inspect_component(id).await?;

        if comp.stage != "arch_approved" && comp.stage != "plan_approved" {
            return Err(RegistryError::InvalidTransition(format!(
                "Cannot plan component '{}' in stage '{}'. Must be 'arch_approved'.",
                id, comp.stage
            )));
        }

        comp.implementation_spec = Some(implementation_spec);
        comp.modification_tasks = tasks;
        comp.stage = "plan_approved".to_string();

        self.repo.save_component(&comp).await?;
        Ok(comp)
    }

    /// Marks a component as implemented:
    /// Invariant: Must be in 'plan_approved' stage.
    pub async fn implement_component(&self, id: &str) -> Result<ComponentSpec, RegistryError> {
        let mut comp = self.inspect_component(id).await?;

        if comp.stage != "plan_approved" {
            return Err(RegistryError::InvalidTransition(format!(
                "Cannot implement component '{}' in stage '{}'. Must be 'plan_approved'.",
                id, comp.stage
            )));
        }

        comp.stage = "implemented".to_string();
        self.repo.save_component(&comp).await?;
        Ok(comp)
    }

    // -------------------------------------------------------------------------
    // 5. Actionable Components
    // -------------------------------------------------------------------------

    /// Returns components ready for their next lifecycle step, prioritized for agents.
    pub async fn get_next_actionable_components(&self) -> Result<Vec<ComponentSummary>, RegistryError> {
        let full = self.repo.load_full().await?;
        let mut actionable = Vec::new();

        for comp in full.components.values() {
            // Components in declared, arch_approved, or plan_approved need work
            if comp.stage == "declared" || comp.stage == "arch_approved" || comp.stage == "plan_approved" {
                actionable.push(ComponentSummary::from_spec(comp));
            }
        }

        Ok(actionable)
    }

    /// Exposes underlying full snapshot for system initialization or sync
    pub async fn load_full(&self) -> Result<SystemArchitecture, RegistryError> {
        self.repo.load_full().await
    }
}
