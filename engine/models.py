from __future__ import annotations
from typing import Dict, List, Optional, Tuple, Any, Union
from pydantic import BaseModel, Field
from enum import StrEnum

# ==========================================
# 1. Atomic Primitive Aliases
# ==========================================
ComponentID = str
NodeID = str
WorkflowTreeName = str
FilePath = str
LineNumber = int
TagName = str
DescriptionText = str
ClassifierType = str


class ComponentStatus(StrEnum):
    NEW = "new"
    EXISTING = "existing"
    MODIFYING = "modifying"
    DEPRECATED = "deprecated"


class LifecycleStage(StrEnum):
    DECLARED = "declared"
    ARCH_APPROVED = "arch_approved"
    PLAN_APPROVED = "plan_approved"
    IMPLEMENTED = "implemented"


# ==========================================
# 2. Composite Named Structures & Unions
# ==========================================
LineRange = Tuple[LineNumber, LineNumber]
OptionalLineRange = Union[LineRange, None]

JSONSchemaDict = Dict[str, Any]
OptionalJSONSchemaDict = Union[JSONSchemaDict, None]

ComponentUpdateFields = Dict[str, Any]
OptionalDescriptionText = Union[DescriptionText, None]


class ComponentTypeRule(BaseModel):
    """Defines structural capability constraints for a dynamically configured component type."""
    allows_properties: bool = Field(
        default=True,
        description="True if components of this type are permitted to define state attributes or fields."
    )
    allows_signature: bool = Field(
        default=True,
        description="True if components of this type are permitted to define executable signatures (inputs, outputs)."
    )
    allowed_parent_types: Optional[List[str]] = Field(
        default=None,
        description="Optional list of component types that are permitted to parent components of this type. If None, any parent type is allowed."
    )


ComponentTypesMap = Dict[str, ComponentTypeRule]


class SideEffect(BaseModel):
    target: TagName = Field(
        ...,
        description="Target tag of the side-effect (e.g., 'db', 'fs', 'network')"
    )
    description: DescriptionText = Field(
        ...,
        description="Description of what this side-effect modifies"
    )


# Explicit named composite collection for Side Effects
SideEffectList = List[SideEffect]


class Location(BaseModel):
    file_path: FilePath = Field(
        ...,
        description="Absolute or relative file path to the definition"
    )
    line_range: OptionalLineRange = Field(
        default=None,
        description="Start and end line numbers"
    )


OptionalLocation = Union[Location, None]


class ModificationTask(BaseModel):
    task: str = Field(
        ...,
        description="The description of the modification task"
    )
    completed: bool = Field(
        default=False,
        description="Whether the modification task is completed"
    )


class InvariantType(StrEnum):
    PRE_CONDITION = "pre_condition"
    POST_CONDITION = "post_condition"
    STATE_INVARIANT = "state_invariant"
    COMPLEXITY = "complexity"


class InvariantRule(BaseModel):
    name: str = Field(
        ...,
        description="Short unique key/name for the invariant rule."
    )
    type: InvariantType = Field(
        default=InvariantType.STATE_INVARIANT,
        description="The category of constraint being enforced."
    )
    description: str = Field(
        ...,
        description="Detailed plain-text explanation of the rule."
    )


class LogicStep(BaseModel):
    sequence: int = Field(
        ...,
        description="Sequential index of the operation."
    )
    name: str = Field(
        ...,
        description="Short summary name of the step."
    )
    description: str = Field(
        ...,
        description="Detailed logical behavior or business rules for this step."
    )
    algorithm: Optional[str] = Field(
        default=None,
        description="Specific algorithm or math to be used (e.g., 'Consistent Hashing')."
    )
    complexity: Optional[str] = Field(
        default=None,
        description="Big-O space or time complexity constraint (e.g., 'O(log N)')."
    )


class ValidationToolDefinition(BaseModel):
    """A project-wide validation tool template (e.g., custom linter, type-checker, test runner)."""
    id: str = Field(
        ...,
        description="Unique identifier for the tool (e.g., 'ruff', 'cargo-clippy', 'tsc')."
    )
    executable: str = Field(
        ...,
        description="The physical binary/program to execute on the system path (e.g., 'ruff', 'mypy', 'cargo')."
    )
    default_args: List[str] = Field(
        default_factory=list,
        description="Default ordered arguments passed to the executable (e.g., ['check', '{targets}'])."
    )


class ComponentValidationRef(BaseModel):
    """Specifies how a component binds to and executes a system validation tool."""
    tool_id: str = Field(
        ...,
        description="Must match a registered ValidationToolDefinition.id."
    )
    targets: List[str] = Field(
        ...,
        description="Specific target files, directories, or modules to validate. Replaces the '{targets}' placeholder."
    )
    args: Optional[List[str]] = Field(
        default=None,
        description="If provided, completely overrides default_args list to prevent argument/flag conflicts."
    )


class ImplementationSpec(BaseModel):
    pattern_or_system: Optional[str] = Field(
        default=None,
        description="The overarching architectural pattern or paradigm (e.g., 'Saga Pattern', 'Event Sourcing')."
    )
    invariants: List[InvariantRule] = Field(
        default_factory=list,
        description="Collection of system/local safety constraints that must be preserved."
    )
    logic_steps: List[LogicStep] = Field(
        default_factory=list,
        description="Sequential sequence of operations detailing the implementation logic."
    )
    validation: List[ComponentValidationRef] = Field(
        default_factory=list,
        description="Sequential list of declarative verification validations to execute."
    )


# ==========================================
# 3. Pure Relational Domain Model Classes
# ==========================================

class Component(BaseModel):
    """The unified relational building block of the software architecture contract."""
    id: ComponentID = Field(
        ...,
        description="Unique alphanumeric identifier (e.g., 'sqlite_user_db_read')."
    )
    name: str = Field(
        ...,
        description="Short human-readable name of the element (e.g., 'read')."
    )
    type: str = Field(
        ...,
        description="Corresponds strictly to a custom key in SystemRegistry.component_types."
    )
    parent_id: Optional[ComponentID] = Field(
        default=None,
        description="Optional ID of the logical parent namespace, establishing hierarchy on-the-fly."
    )
    implements_id: Optional[ComponentID] = Field(
        default=None,
        description="Optional ID of the abstract operation or interface component this element implements."
    )
    description: DescriptionText = Field(
        ...,
        description="Detailed description of what the component does."
    )
    status: ComponentStatus = Field(
        default=ComponentStatus.NEW,
        description="Implementation or design lifecycle status."
    )
    stage: LifecycleStage = Field(
        default=LifecycleStage.DECLARED,
        description="The current state/stage in the development lifecycle."
    )
    location: OptionalLocation = Field(
        default=None,
        description="Physical file path and line ranges if the status is 'existing'."
    )
    
    # State Definition
    properties: OptionalJSONSchemaDict = Field(
        default=None,
        description="JSON Schema for fields, attributes, config parameters, or enum keys."
    )
    
    # Executable Signature Definition
    inputs: OptionalJSONSchemaDict = Field(
        default=None,
        description="JSON Schema specifying function parameters or arguments."
    )
    outputs: OptionalJSONSchemaDict = Field(
        default=None,
        description="JSON Schema specifying function return values."
    )
    side_effects: SideEffectList = Field(
        default_factory=list,
        description="Declared system-level infrastructure side-effects."
    )
    implementation_spec: Optional[ImplementationSpec] = Field(
        default=None,
        description="The abstract implementation specification (Stage 1 Design Output)."
    )
    modification_tasks: List[ModificationTask] = Field(
        default_factory=list,
        description="Itemized list of planned or completed modification tasks."
    )


ComponentRegistryMap = Dict[ComponentID, Component]


class UsageNode(BaseModel):
    node_id: NodeID = Field(
        ...,
        description="Unique ID for this use site/node"
    )
    caller_id: ComponentID = Field(
        ...,
        description="ID of the registered component initiating the call."
    )
    component_id: ComponentID = Field(
        ...,
        description="ID of the component being referenced"
    )
    description: OptionalDescriptionText = Field(
        default=None,
        description="Context/explanation of this usage"
    )
    
    # Flattened expected interface fields
    expected_inputs: OptionalJSONSchemaDict = Field(
        default=None,
        description="The inputs expected at this call-site"
    )
    expected_outputs: OptionalJSONSchemaDict = Field(
        default=None,
        description="The outputs expected at this call-site"
    )
    expected_side_effects: SideEffectList = Field(
        default_factory=list,
        description="The side-effects expected at this call-site"
    )
    
    dependencies: UsageNodeList = Field(
        default_factory=list,
        description="Sub-calls or dependencies triggered by this use-site"
    )


UsageNodeList = List[UsageNode]
UsageTreeRegistryMap = Dict[WorkflowTreeName, UsageNode]


class SystemRegistry(BaseModel):
    components: ComponentRegistryMap = Field(
        default_factory=dict,
        description="Registry of all defined components"
    )
    usage_trees: UsageTreeRegistryMap = Field(
        default_factory=dict,
        description="Registry of named usage/dependency trees"
    )
    component_types: ComponentTypesMap = Field(
        default_factory=dict,
        description="Dynamic mapping of component types to their structural capability rules."
    )
    validation_tools: Dict[str, ValidationToolDefinition] = Field(
        default_factory=dict,
        description="Global registry of available validation tools."
    )


# Rebuild models for recursive type reference handling in Pydantic v2
UsageNode.model_rebuild()
SystemRegistry.model_rebuild()
