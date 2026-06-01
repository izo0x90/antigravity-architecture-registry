from typing import List, Dict, Any, Union, Tuple, Optional
from .models import UsageNode, Component, ComponentRegistryMap, ComponentTypesMap, SideEffectList, SystemRegistry
from .exceptions import CompatibilityError

ErrorMessagesList = List[str]
WarningMessagesList = List[str]
CompatibilityErrorList = List[CompatibilityError]


def compare_schemas(expected: Union[dict, None], actual: Union[dict, None]) -> ErrorMessagesList:
    """Recursively checks if expected schema structurally satisfies the actual schema rules."""
    errors: ErrorMessagesList = []
    if not actual:
        return errors  # Actual imposes no rules / is open
    if not expected:
        errors.append("Expected interface schema is missing but actual is defined.")
        return errors

    # Handle anyOf in expected schema (all options in expected must be compatible with actual)
    if "anyOf" in expected:
        for i, sub_expected in enumerate(expected["anyOf"]):
            if isinstance(sub_expected, dict):
                sub_errs = compare_schemas(sub_expected, actual)
                if sub_errs:
                    errors.append(f"Expected option {i} is incompatible: {'; '.join(sub_errs)}")
        return errors

    # Handle anyOf in actual schema (expected must be compatible with at least one option in actual)
    if "anyOf" in actual:
        anyof_errors = []
        for sub_actual in actual["anyOf"]:
            if isinstance(sub_actual, dict):
                sub_errs = compare_schemas(expected, sub_actual)
                if not sub_errs:
                    return []  # Found a match!
                anyof_errors.append(f"({'; '.join(sub_errs)})")
        errors.append(f"none of the actual anyOf options were satisfied: {', '.join(anyof_errors)}")
        return errors

    actual_type: Any = actual.get("type")
    expected_type: Any = expected.get("type")
    
    if actual_type != expected_type:
        errors.append(f"Type mismatch: expected '{expected_type}', actual '{actual_type}'")
        return errors
        
    if actual_type == "object":
        actual_props: dict = actual.get("properties", {})
        expected_props: dict = expected.get("properties", {})
        actual_required: list = actual.get("required", [])
        
        # 1. Check all required fields of actual are present in expected
        for req in actual_required:
            if req not in expected_props:
                errors.append(f"Missing required property '{req}'")
                
        # 2. Check each overlapping property type compatibility
        for name, actual_prop in actual_props.items():
            if name in expected_props and isinstance(actual_prop, dict) and isinstance(expected_props[name], dict):
                sub_errors = compare_schemas(expected_props[name], actual_prop)
                for err in sub_errors:
                    errors.append(f"Property '{name}': {err}")
                    
    elif actual_type == "array":
        actual_items: Any = actual.get("items")
        expected_items: Any = expected.get("items")
        if isinstance(actual_items, dict) and isinstance(expected_items, dict):
            sub_errors = compare_schemas(expected_items, actual_items)
            for err in sub_errors:
                errors.append(f"Array items: {err}")
                
    return errors


class CompatibilityResult:
    """Structured result of compatibility checks containing any warnings or strict errors."""

    def __init__(self) -> None:
        self.is_compatible: bool = True
        self.errors: ErrorMessagesList = []
        self.warnings: WarningMessagesList = []


def resolve_refs(
    schema: Any, 
    components: ComponentRegistryMap, 
    component_types: ComponentTypesMap,
    visited: Optional[set[str]] = None
) -> Any:
    """Recursively resolves custom object reference titles using dynamic type capabilities.

    Raises:
        CompatibilityError: If a referenced custom type title is not found or forms a cycle.
    """
    if visited is None:
        visited = set()

    if not isinstance(schema, dict):
        return schema
    
    title = schema.get("title")
    if title is not None:
        if title not in components:
            raise CompatibilityError(
                node_id="",
                component_id=title,
                details=f"Referenced custom type '{title}' was not found in the registry."
            )
        
        if title in visited:
            raise CompatibilityError(
                node_id="",
                component_id=title,
                details=f"Circular reference loop detected resolving custom type '{title}'."
            )
        
        visited.add(title)
        comp = components[title]
        rule = component_types.get(comp.type)
        
        if rule and rule.allows_properties and not rule.allows_signature:
            properties_schema = comp.properties or {}
            return resolve_refs(properties_schema, components, component_types, set(visited))
            
    return _resolve_sub_properties(schema, components, component_types, visited)


def _resolve_sub_properties(
    schema: dict, 
    components: ComponentRegistryMap, 
    component_types: ComponentTypesMap,
    visited: set[str]
) -> dict:
    """Helper utility to traverse nested schema collections recursively."""
    resolved = dict(schema)
    if "properties" in resolved and isinstance(resolved["properties"], dict):
        resolved["properties"] = {
            k: resolve_refs(v, components, component_types, set(visited)) 
            for k, v in resolved["properties"].items()
        }
    if "items" in resolved:
        resolved["items"] = resolve_refs(resolved["items"], components, component_types, set(visited))
    if "anyOf" in resolved and isinstance(resolved["anyOf"], list):
        resolved["anyOf"] = [
            resolve_refs(item, components, component_types, set(visited)) 
            for item in resolved["anyOf"]
        ]
    return resolved


def resolve_implements_signature(
    comp: Component, 
    components: ComponentRegistryMap
) -> Tuple[Any, Any]:
    """Retrieves inputs/outputs, falling back to implemented interface/operation contracts recursively."""
    inputs = comp.inputs
    outputs = comp.outputs
    
    curr = comp
    visited = {comp.id}
    while (inputs is None or outputs is None) and curr.implements_id:
        impl_id = curr.implements_id
        if impl_id in visited:
            break
        visited.add(impl_id)
        
        parent = components.get(impl_id)
        if not parent:
            break
            
        if inputs is None:
            inputs = parent.inputs
        if outputs is None:
            outputs = parent.outputs
        curr = parent
        
    return inputs, outputs


class ArchitectureValidator:
    """Verifies interface matching and side-effects across the usage tree."""

    @classmethod
    def validate_registry(
        cls, 
        registry: SystemRegistry
    ) -> CompatibilityErrorList:
        """Runs comprehensive, registry-wide static analysis to enforce all structural invariants.
        
        Verifies:
          1. Component Type Validity
          2. Parent and Implements existence
          3. Parenting & Implements Cycle Detection
          4. Capability Enforcement (allows_properties, allows_signature rules)
          5. Dynamic Parenting constraints (allowed_parent_types rules)
          6. Interface Contract Realization (implements_id matching)
          7. Custom Reference Resolution and schema loop protection
        """
        errors: CompatibilityErrorList = []
        components = registry.components
        comp_types = registry.component_types

        for comp_id, comp in components.items():
            # 1. Component Type Validity
            if comp.type not in comp_types:
                errors.append(
                    CompatibilityError(
                        node_id="",
                        component_id=comp_id,
                        details=f"Component '{comp_id}' has unregistered type '{comp.type}'."
                    )
                )
                continue  # Skip further checks on invalid types to avoid key-errors

            # 2. Parent Existence
            if comp.parent_id is not None and comp.parent_id not in components:
                errors.append(
                    CompatibilityError(
                        node_id="",
                        component_id=comp_id,
                        details=f"Parent component '{comp.parent_id}' referenced by component '{comp_id}' does not exist."
                    )
                )

            # 3. Implements Existence
            if comp.implements_id is not None and comp.implements_id not in components:
                errors.append(
                    CompatibilityError(
                        node_id="",
                        component_id=comp_id,
                        details=f"Implemented component '{comp.implements_id}' referenced by component '{comp_id}' does not exist."
                    )
                )

            # 4. Cycle Detection (Parenting)
            try:
                visited_parents = {comp_id}
                curr_parent_id = comp.parent_id
                while curr_parent_id is not None:
                    if curr_parent_id in visited_parents:
                        raise CompatibilityError(
                            node_id="",
                            component_id=comp_id,
                            details=f"Cyclic parenting relationship detected: '{comp_id}' forms a loop via '{curr_parent_id}'."
                        )
                    visited_parents.add(curr_parent_id)
                    parent_comp = components.get(curr_parent_id)
                    if not parent_comp:
                        break  # Handled by existence check above
                    curr_parent_id = parent_comp.parent_id
            except CompatibilityError as exc:
                errors.append(exc)

            # 5. Cycle Detection (Implements)
            try:
                visited_impls = {comp_id}
                curr_impl_id = comp.implements_id
                while curr_impl_id is not None:
                    if curr_impl_id in visited_impls:
                        raise CompatibilityError(
                            node_id="",
                            component_id=comp_id,
                            details=f"Cyclic implements relationship detected: '{comp_id}' forms a loop via '{curr_impl_id}'."
                        )
                    visited_impls.add(curr_impl_id)
                    impl_comp = components.get(curr_impl_id)
                    if not impl_comp:
                        break  # Handled by existence check above
                    curr_impl_id = impl_comp.implements_id
            except CompatibilityError as exc:
                errors.append(exc)

            # 6. Capability & Parenting constraints
            try:
                cls.verify_capabilities(comp, comp_types)
            except CompatibilityError as exc:
                errors.append(exc)

            try:
                cls.verify_parenting(comp, components, comp_types)
            except CompatibilityError as exc:
                errors.append(exc)

            # 7. Interface Contract Realization Check
            if comp.implements_id is not None and comp.implements_id in components:
                interface_comp = components[comp.implements_id]
                
                # Check Inputs Compatibility only if both explicitly define inputs schemas
                if interface_comp.inputs is not None and comp.inputs is not None:
                    input_errs = compare_schemas(interface_comp.inputs, comp.inputs)
                    for err in input_errs:
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Interface contract mismatch with '{comp.implements_id}' on inputs: {err}"
                            )
                        )
                
                # Check Outputs Compatibility only if both explicitly define outputs schemas
                if interface_comp.outputs is not None and comp.outputs is not None:
                    output_errs = compare_schemas(comp.outputs, interface_comp.outputs)
                    for err in output_errs:
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Interface contract mismatch with '{comp.implements_id}' on outputs: {err}"
                            )
                        )

                # Check Side Effects compatibility (all side effects triggered by component must be allowed by interface)
                actual_targets = {se.target for se in comp.side_effects}
                expected_targets = {se.target for se in interface_comp.side_effects}
                for target in actual_targets:
                    if target not in expected_targets:
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Undeclared side-effect target '{target}' triggered by component; not permitted by interface contract '{comp.implements_id}'."
                            )
                        )

            # 8. Schema References Verification (properties, inputs, outputs)
            for field_name, schema in [("properties", comp.properties), ("inputs", comp.inputs), ("outputs", comp.outputs)]:
                if schema is not None:
                    try:
                        resolve_refs(schema, components, comp_types)
                    except CompatibilityError as exc:
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Schema reference error in '{field_name}': {exc.details}"
                            )
                        )

            # 9. Implementation Spec Validation
            if comp.implementation_spec is not None:
                spec = comp.implementation_spec
                
                # Check contiguity of sequence indices
                if spec.logic_steps:
                    steps = sorted(spec.logic_steps, key=lambda s: s.sequence)
                    sequences = [s.sequence for s in steps]
                    
                    # 1. Unique checks
                    if len(sequences) != len(set(sequences)):
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Component '{comp_id}' logic steps must have unique sequence indices. Found duplicates: {sequences}."
                            )
                        )
                    
                    # 2. Contiguous 1 to N check
                    n = len(sequences)
                    expected_seq = list(range(1, n + 1))
                    if sequences != expected_seq:
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Component '{comp_id}' logic steps must be sequentially contiguous starting from 1. Found sequence indices {sequences}, expected {expected_seq}."
                            )
                        )

            # 10. Interface Invariant Rule Compatibility
            if comp.implements_id is not None and comp.implements_id in components:
                interface_comp = components[comp.implements_id]
                if interface_comp.implementation_spec is not None and interface_comp.implementation_spec.invariants:
                    parent_invariants = {inv.name: inv for inv in interface_comp.implementation_spec.invariants}
                    
                    if comp.implementation_spec is not None:
                        child_invariants = {inv.name: inv for inv in comp.implementation_spec.invariants}
                        
                        for name, p_inv in parent_invariants.items():
                            if name in child_invariants:
                                c_inv = child_invariants[name]
                                if c_inv.type != p_inv.type:
                                    errors.append(
                                        CompatibilityError(
                                            node_id="",
                                            component_id=comp_id,
                                            details=f"Invariant rule '{name}' type mismatch with parent interface '{comp.implements_id}': parent type '{p_inv.type}', actual type '{c_inv.type}'."
                                        )
                                    )

        return errors

    @staticmethod
    def verify_capabilities(
        comp: Component, 
        component_types: ComponentTypesMap
    ) -> None:
        """Structurally enforces capability rules on a component, throwing loud custom errors."""
        rule = component_types.get(comp.type)
        if not rule:
            raise CompatibilityError(
                node_id="",
                component_id=comp.id,
                details=f"Component '{comp.id}' has unregistered type '{comp.type}'"
            )
            
        if not rule.allows_properties and comp.properties is not None:
            raise CompatibilityError(
                node_id="",
                component_id=comp.id,
                details=f"Type '{comp.type}' does not permit properties on component '{comp.id}'"
            )
            
        if not rule.allows_signature and (
            comp.inputs is not None or comp.outputs is not None or comp.side_effects
        ):
            raise CompatibilityError(
                node_id="",
                component_id=comp.id,
                details=f"Type '{comp.type}' does not permit executable signatures on component '{comp.id}'"
            )

    @classmethod
    def verify_parenting(
        cls,
        comp: Component,
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap
    ) -> None:
        """Enforces parent-type constraints dynamically, preventing invalid hierarchies."""
        if comp.parent_id is None:
            return
            
        parent = components.get(comp.parent_id)
        if not parent:
            # Existence of parent_id is checked in engine.py before calling this
            return
            
        rule = component_types.get(comp.type)
        if not rule or rule.allowed_parent_types is None:
            return
            
        if parent.type not in rule.allowed_parent_types:
            raise CompatibilityError(
                node_id="",
                component_id=comp.id,
                details=(
                    f"Parenting violation: Component '{comp.id}' of type '{comp.type}' "
                    f"cannot be parented by '{parent.id}' of type '{parent.type}'. "
                    f"Allowed parent types for '{comp.type}' are: {rule.allowed_parent_types}."
                )
            )

    @classmethod
    def verify_compatibility(
        cls,
        expected_inputs: Any,
        expected_outputs: Any,
        expected_side_effects: SideEffectList,
        actual_inputs: Any,
        actual_outputs: Any,
        actual_side_effects: SideEffectList,
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap,
    ) -> CompatibilityResult:
        """Checks if the expected interface at a use-site is compatible with the actual definition."""
        result = CompatibilityResult()
        
        # Resolve references recursively
        resolved_expected_inputs = resolve_refs(expected_inputs, components, component_types)
        resolved_actual_inputs = resolve_refs(actual_inputs, components, component_types)
        resolved_expected_outputs = resolve_refs(expected_outputs, components, component_types)
        resolved_actual_outputs = resolve_refs(actual_outputs, components, component_types)

        # 1. Compare Inputs (contravariant/inputs check: caller must satisfy component requirements)
        input_errors = compare_schemas(resolved_expected_inputs, resolved_actual_inputs)
        for err in input_errors:
            result.errors.append(f"Input mismatch: {err}")
            
        # 2. Compare Outputs (covariant/outputs check: component must satisfy caller expectations)
        output_errors = compare_schemas(resolved_actual_outputs, resolved_expected_outputs)
        for err in output_errors:
            result.errors.append(f"Output mismatch: {err}")
            
        # 3. Compare Side Effects
        expected_targets = {se.target for se in expected_side_effects}
        actual_targets = {se.target for se in actual_side_effects}
        for target in actual_targets:
            if target not in expected_targets:
                result.errors.append(f"Undeclared side-effect target '{target}' triggered by component.")
                
        if result.errors:
            result.is_compatible = False
            
        return result

    @classmethod
    def validate_usage_node(
        cls, 
        node: UsageNode, 
        components: ComponentRegistryMap,
        component_types: ComponentTypesMap
    ) -> CompatibilityErrorList:
        """Recursively checks a usage node and all its children.

        Returns:
            CompatibilityErrorList: List of all structural/interface mismatches found.
        """
        errors: CompatibilityErrorList = []
        
        # 1. Fetch and verify caller component
        caller = components.get(node.caller_id)
        if not caller:
            errors.append(
                CompatibilityError(
                    node_id=node.node_id,
                    component_id=node.caller_id,
                    details=f"Caller component '{node.caller_id}' is referenced but not registered in the system."
                )
            )
            
        # 2. Fetch and verify target (callee) component
        callee = components.get(node.component_id)
        if not callee:
            errors.append(
                CompatibilityError(
                    node_id=node.node_id,
                    component_id=node.component_id,
                    details=f"Target component '{node.component_id}' is referenced but not registered."
                )
            )
            
        # 3. Perform contract checks only if both exist
        if caller and callee:
            # Check compatibility if expected fields are defined
            if node.expected_inputs is not None or node.expected_outputs is not None or node.expected_side_effects:
                # Resolve actual interface signature considering inheritance
                actual_inputs, actual_outputs = resolve_implements_signature(callee, components)
                
                compat = cls.verify_compatibility(
                    expected_inputs=node.expected_inputs,
                    expected_outputs=node.expected_outputs,
                    expected_side_effects=node.expected_side_effects,
                    actual_inputs=actual_inputs,
                    actual_outputs=actual_outputs,
                    actual_side_effects=callee.side_effects,
                    components=components,
                    component_types=component_types,
                )
                if not compat.is_compatible:
                    details_str = "; ".join(compat.errors)
                    errors.append(
                        CompatibilityError(
                            node_id=node.node_id,
                            component_id=node.component_id,
                            details=details_str
                        )
                    )
                    
        # 4. Recursively check child/dependency use-sites
        for child in node.dependencies:
            errors.extend(cls.validate_usage_node(child, components, component_types))
            
        return errors
