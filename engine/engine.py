import json
import os
from pathlib import Path
from typing import Dict, Union, Any
from pydantic import ValidationError

from .exceptions import RegistryError, ComponentNotFoundError, DuplicateComponentError
from .models import (
    SystemRegistry,
    Component,
    UsageNode,
    ComponentID,
    NodeID,
    WorkflowTreeName,
    ComponentUpdateFields,
    ComponentTypeRule,
)

# Constants to avoid magic values
UTF8_ENCODING: str = "utf-8"
TEMP_FILE_SUFFIX: str = ".tmp"
JSON_INDENT_LEVEL: int = 2

OptionalNodeID = Union[NodeID, None]
UsageNodeUpdateFields = Dict[str, Any]


def migrate_usage_node(node_dict: dict) -> dict:
    """Recursively migrates a legacy usage node to the flattened schema."""
    if "expected_interface" in node_dict:
        exp_int = node_dict.pop("expected_interface")
        if exp_int is not None:
            node_dict["expected_inputs"] = exp_int.get("inputs")
            node_dict["expected_outputs"] = exp_int.get("outputs")
            node_dict["expected_side_effects"] = exp_int.get("side_effects", [])
    
    if "dependencies" in node_dict and isinstance(node_dict["dependencies"], list):
        node_dict["dependencies"] = [migrate_usage_node(child) for child in node_dict["dependencies"]]
    return node_dict


def migrate_component(comp_dict: dict) -> dict:
    """Migrates a legacy component to the flattened relational schema."""
    if "type" in comp_dict and "category" not in comp_dict:
        return comp_dict
        
    interface = comp_dict.pop("interface", {}) or {}
    inputs = interface.get("inputs")
    outputs = interface.get("outputs")
    side_effects = interface.get("side_effects", [])
    
    category = comp_dict.pop("category", "module")
    classifier = comp_dict.pop("classifier", {}) or {}
    sub_type = classifier.get("type")
    
    if sub_type:
        comp_type = sub_type
    elif category == "callable":
        comp_type = "function"
    elif category == "container":
        comp_type = "module"
    else:
        comp_type = category
        
    comp_dict["type"] = comp_type
    comp_dict["inputs"] = inputs
    comp_dict["outputs"] = outputs
    comp_dict["side_effects"] = side_effects
    
    # For property-allowing types, map existing fields to properties
    if comp_type in ("class", "data_object", "enum"):
        comp_dict["properties"] = inputs or outputs
        if comp_type != "class":
            comp_dict["inputs"] = None
            comp_dict["outputs"] = None
            comp_dict["side_effects"] = []
            
    return comp_dict


class RegistryEngine:
    """Manages CRUD operations and state persistence for the Architecture Registry."""

    def __init__(self, registry_file_path: Path):
        self.file_path: Path = registry_file_path
        self.registry: SystemRegistry = SystemRegistry()

    def seed_component_types(self) -> None:
        """Seeds dynamic type capabilities. Zero magic logic resides in the engine itself."""
        self.registry.component_types = {
            "module": ComponentTypeRule(
                allows_properties=False,
                allows_signature=False,
                allowed_parent_types=["module"]
            ),
            "class": ComponentTypeRule(
                allows_properties=True,
                allows_signature=False,
                allowed_parent_types=["module"]
            ),
            "interface": ComponentTypeRule(
                allows_properties=False,
                allows_signature=False,
                allowed_parent_types=["module"]
            ),
            "function": ComponentTypeRule(
                allows_properties=False,
                allows_signature=True,
                allowed_parent_types=["module", "class"]
            ),
            "operation": ComponentTypeRule(
                allows_properties=False,
                allows_signature=True,
                allowed_parent_types=["interface", "module"]
            ),
            "data_object": ComponentTypeRule(
                allows_properties=True,
                allows_signature=False,
                allowed_parent_types=["module", "class"]
            ),
            "enum": ComponentTypeRule(
                allows_properties=True,
                allows_signature=False,
                allowed_parent_types=["module", "class"]
            ),
            "actor": ComponentTypeRule(
                allows_properties=False,
                allows_signature=False,
                allowed_parent_types=[]
            ),
            "system_trigger": ComponentTypeRule(
                allows_properties=False,
                allows_signature=False,
                allowed_parent_types=[]
            ),
        }

    def init(self) -> None:
        """Initializes a brand new, empty registry file.

        Raises:
            RegistryError: If the file already exists or cannot be created.
        """
        if self.file_path.exists():
            raise RegistryError(f"Registry file at '{self.file_path}' already exists.")
        
        self.registry = SystemRegistry()
        self.seed_component_types()
        self.save()

    def load(self) -> None:
        """Loads, migrates and parses the registry file with strict Pydantic validation.

        Raises:
            RegistryError: If the file does not exist, JSON deserialization fails, or Pydantic validation fails.
        """
        if not self.file_path.exists():
            # Auto-init instead of crashing is extremely helpful in integration test flows
            self.init()
            return

        try:
            with open(self.file_path, "r", encoding=UTF8_ENCODING) as f:
                raw_data: dict = json.load(f)

            # Support dynamic migration of old registries
            if isinstance(raw_data, dict):
                if "components" in raw_data and isinstance(raw_data["components"], dict):
                    raw_data["components"] = {
                        k: migrate_component(v) for k, v in raw_data["components"].items()
                    }
                if "usage_trees" in raw_data and isinstance(raw_data["usage_trees"], dict):
                    raw_data["usage_trees"] = {
                        k: migrate_usage_node(v) for k, v in raw_data["usage_trees"].items()
                    }

            # Strict parsing and validation via Pydantic
            self.registry = SystemRegistry.model_validate(raw_data)

            # Ensure component types are seeded if loading from an empty or old file
            if not self.registry.component_types:
                self.seed_component_types()

        except json.JSONDecodeError as exc:
            raise RegistryError(
                f"Failed to parse registry file at '{self.file_path}' as valid JSON: {str(exc)}"
            ) from exc

        except ValidationError as exc:
            raise RegistryError(
                f"Registry file at '{self.file_path}' is structurally invalid:\n{str(exc)}"
            ) from exc

        except OSError as exc:
            raise RegistryError(
                f"Failed to read registry file at '{self.file_path}' due to an I/O error: {str(exc)}"
            ) from exc

    def save(self) -> None:
        """Saves current state to the registry file atomically."""
        temp_file_path: Path = self.file_path.with_suffix(
            self.file_path.suffix + TEMP_FILE_SUFFIX
        )

        try:
            serialized_json: str = self.registry.model_dump_json(
                indent=JSON_INDENT_LEVEL
            )

            with open(temp_file_path, "w", encoding=UTF8_ENCODING) as f:
                f.write(serialized_json)

            os.replace(temp_file_path, self.file_path)

        except OSError as exc:
            if temp_file_path.exists():
                try:
                    temp_file_path.unlink()
                except OSError:
                    pass

            raise RegistryError(
                f"Failed to atomically write registry file to '{self.file_path}': {str(exc)}"
            ) from exc

    # --- Component CRUD ---

    def add_component(self, component: Component) -> None:
        """Adds a component to the registry.

        Raises:
            DuplicateComponentError: If a component with the same ID already exists.
            RegistryError: If structural or capability rules are violated.
        """
        if component.id in self.registry.components:
            raise DuplicateComponentError(component.id)
            
        if component.type not in self.registry.component_types:
            raise RegistryError(f"Component '{component.id}' has unregistered type '{component.type}'")
            
        if component.parent_id is not None:
            if component.parent_id not in self.registry.components:
                raise RegistryError(f"Parent component '{component.parent_id}' does not exist.")
            if component.parent_id == component.id:
                raise RegistryError(f"Component '{component.id}' cannot be its own parent.")
                
        if component.implements_id is not None:
            if component.implements_id not in self.registry.components:
                raise RegistryError(f"Implements component '{component.implements_id}' does not exist.")
                
        # Run capability guardrail verification
        from .validator import ArchitectureValidator
        ArchitectureValidator.verify_capabilities(component, self.registry.component_types)
        ArchitectureValidator.verify_parenting(component, self.registry.components, self.registry.component_types)
        
        self.registry.components[component.id] = component

    def update_component(
        self, component_id: ComponentID, updates: ComponentUpdateFields
    ) -> None:
        """Updates specific attributes of an existing component.

        Raises:
            ComponentNotFoundError: If the component does not exist.
            RegistryError: If update validation fails or capability rules are violated.
        """
        if component_id not in self.registry.components:
            raise ComponentNotFoundError(component_id)
        
        comp = self.registry.components[component_id]
        comp_dict = comp.model_dump()
        
        for k, v in updates.items():
            if isinstance(v, dict) and k in comp_dict and isinstance(comp_dict[k], dict):
                comp_dict[k].update(v)
            else:
                comp_dict[k] = v
                
        try:
            updated_comp = Component.model_validate(comp_dict)
        except ValidationError as exc:
            raise RegistryError(f"Failed to update component '{component_id}' due to validation error: {str(exc)}") from exc
        
        if updated_comp.type not in self.registry.component_types:
            raise RegistryError(f"Component '{updated_comp.id}' has unregistered type '{updated_comp.type}'")
            
        if updated_comp.parent_id is not None:
            if updated_comp.parent_id not in self.registry.components:
                raise RegistryError(f"Parent component '{updated_comp.parent_id}' does not exist.")
            if updated_comp.parent_id == updated_comp.id:
                raise RegistryError(f"Component '{updated_comp.id}' cannot be its own parent.")
                
        if updated_comp.implements_id is not None:
            if updated_comp.implements_id not in self.registry.components:
                raise RegistryError(f"Implements component '{updated_comp.implements_id}' does not exist.")
                
        # Run capability guardrail verification
        from .validator import ArchitectureValidator
        ArchitectureValidator.verify_capabilities(updated_comp, self.registry.component_types)
        ArchitectureValidator.verify_parenting(updated_comp, self.registry.components, self.registry.component_types)
        
        self.registry.components[component_id] = updated_comp

    def delete_component(self, component_id: ComponentID) -> None:
        """Removes a component.

        Raises:
            ComponentNotFoundError: If the component does not exist.
            RegistryError: If the component is still referenced by any active usage node or child component.
        """
        if component_id not in self.registry.components:
            raise ComponentNotFoundError(component_id)
            
        # Check if any component references this component as parent or implements
        for other_id, other_comp in self.registry.components.items():
            if other_comp.parent_id == component_id:
                raise RegistryError(
                    f"Cannot delete component '{component_id}'; it is referenced as parent by component '{other_id}'."
                )
            if other_comp.implements_id == component_id:
                raise RegistryError(
                    f"Cannot delete component '{component_id}'; it is implemented by component '{other_id}'."
                )

        def _is_component_referenced(node: UsageNode) -> bool:
            if node.component_id == component_id:
                return True
            for child in node.dependencies:
                if _is_component_referenced(child):
                    return True
            return False

        # Scan all usage trees to prevent breaking dependency chains
        for tree_name, root_node in self.registry.usage_trees.items():
            if _is_component_referenced(root_node):
                raise RegistryError(
                    f"Cannot delete component '{component_id}'; it is referenced by usage tree '{tree_name}'."
                )
                
        del self.registry.components[component_id]

    # --- Usage Tree CRUD ---

    def add_usage_node(
        self,
        tree_name: WorkflowTreeName,
        parent_node_id: OptionalNodeID,
        node: UsageNode,
    ) -> None:
        """Inserts a usage node into the designated workflow dependency tree."""
        if node.component_id not in self.registry.components:
            raise ComponentNotFoundError(node.component_id)

        if parent_node_id is None:
            if tree_name in self.registry.usage_trees:
                raise RegistryError(f"Root node already exists for usage tree '{tree_name}'. Use update or delete instead.")
            self.registry.usage_trees[tree_name] = node
            return

        if tree_name not in self.registry.usage_trees:
            raise RegistryError(f"Usage tree '{tree_name}' does not exist.")

        root = self.registry.usage_trees[tree_name]

        def _add_child_recursive(curr: UsageNode) -> bool:
            if curr.node_id == parent_node_id:
                for child in curr.dependencies:
                    if child.node_id == node.node_id:
                        raise RegistryError(f"Usage node with ID '{node.node_id}' already exists in tree '{tree_name}'.")
                curr.dependencies.append(node)
                return True
            for child in curr.dependencies:
                if _add_child_recursive(child):
                    return True
            return False

        if not _add_child_recursive(root):
            raise RegistryError(f"Parent node '{parent_node_id}' not found in tree '{tree_name}'.")

    def update_usage_node(
        self,
        tree_name: WorkflowTreeName,
        node_id: NodeID,
        updates: UsageNodeUpdateFields,
    ) -> None:
        """Updates specific attributes of an existing usage node."""
        if tree_name not in self.registry.usage_trees:
            raise RegistryError(f"Usage tree '{tree_name}' does not exist.")

        root = self.registry.usage_trees[tree_name]

        def _update_node_recursive(curr: UsageNode) -> bool:
            if curr.node_id == node_id:
                curr_dict = curr.model_dump()
                for k, v in updates.items():
                    if isinstance(v, dict) and k in curr_dict and isinstance(curr_dict[k], dict):
                        curr_dict[k].update(v)
                    else:
                        curr_dict[k] = v
                try:
                    updated = UsageNode.model_validate(curr_dict)
                except ValidationError as exc:
                    raise RegistryError(f"Invalid updates for node '{node_id}': {str(exc)}") from exc
                
                curr.description = updated.description
                curr.expected_inputs = updated.expected_inputs
                curr.expected_outputs = updated.expected_outputs
                curr.expected_side_effects = updated.expected_side_effects
                if "component_id" in updates:
                    if updated.component_id not in self.registry.components:
                        raise ComponentNotFoundError(updated.component_id)
                    curr.component_id = updated.component_id
                if "dependencies" in updates:
                    curr.dependencies = updated.dependencies
                return True
                
            for child in curr.dependencies:
                if _update_node_recursive(child):
                    return True
            return False

        if not _update_node_recursive(root):
            raise RegistryError(f"Usage node '{node_id}' not found in tree '{tree_name}'.")

    def delete_usage_node(self, tree_name: WorkflowTreeName, node_id: NodeID) -> None:
        """Removes a usage node and all its child sub-calls from the tree."""
        if tree_name not in self.registry.usage_trees:
            raise RegistryError(f"Usage tree '{tree_name}' does not exist.")

        root = self.registry.usage_trees[tree_name]

        if root.node_id == node_id:
            del self.registry.usage_trees[tree_name]
            return

        def _delete_node_recursive(curr: UsageNode) -> bool:
            for i, child in enumerate(curr.dependencies):
                if child.node_id == node_id:
                    curr.dependencies.pop(i)
                    return True
                if _delete_node_recursive(child):
                    return True
            return False

        if not _delete_node_recursive(root):
            raise RegistryError(f"Usage node '{node_id}' not found in tree '{tree_name}'.")
