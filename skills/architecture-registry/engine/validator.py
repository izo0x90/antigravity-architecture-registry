from typing import List, Dict, Any, Union, Tuple, Optional
from .models import UsageNode, Component, ComponentRegistryMap, ComponentTypesMap, SideEffectList, SystemRegistry, LifecycleStage
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

            # 11. Validation Tools Verification
            if comp.implementation_spec is not None and comp.implementation_spec.validation:
                for ref in comp.implementation_spec.validation:
                    if ref.tool_id not in registry.validation_tools:
                        errors.append(
                            CompatibilityError(
                                node_id="",
                                component_id=comp_id,
                                details=f"Component '{comp_id}' references unregistered validation tool ID '{ref.tool_id}'."
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

        if not rule.allows_implementation_spec and comp.implementation_spec is not None:
            raise CompatibilityError(
                node_id="",
                component_id=comp.id,
                details=f"Type '{comp.type}' does not permit implementation specifications on component '{comp.id}'"
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
        else:
            caller_rule = component_types.get(caller.type)
            if caller_rule and not caller_rule.is_executable_caller:
                errors.append(
                    CompatibilityError(
                        node_id=node.node_id,
                        component_id=node.caller_id,
                        details=f"Caller component '{node.caller_id}' of type '{caller.type}' is a container/namespace and cannot directly execute dependencies."
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


def get_component_dependencies(comp: Component) -> List[str]:
    """Extracts all explicit and implicit signature/parent dependencies of a component."""
    deps = []
    if comp.implements_id:
        deps.append(comp.implements_id)
    if comp.parent_id:
        deps.append(comp.parent_id)
    
    # Traverse schemas for titles (custom object references)
    def extract_titles(schema: Any) -> List[str]:
        titles = []
        if isinstance(schema, dict):
            title = schema.get("title")
            if title:
                titles.append(title)
            for v in schema.values():
                titles.extend(extract_titles(v))
        elif isinstance(schema, list):
            for item in schema:
                titles.extend(extract_titles(item))
        return titles

    for schema in [comp.properties, comp.inputs, comp.outputs]:
        if schema:
            deps.extend(extract_titles(schema))
            
    # Deduplicate and filter out self-references
    return list(set([d for d in deps if d != comp.id]))


def topological_sort_components(components: Dict[str, Component]) -> List[str]:
    """Returns a list of component IDs in topological order (bottom-up dependencies first)."""
    adj = {cid: [] for cid in components}
    in_degree = {cid: 0 for cid in components}
    
    for cid, comp in components.items():
        deps = get_component_dependencies(comp)
        for dep in deps:
            if dep in components:
                adj[dep].append(cid)
                in_degree[cid] += 1
                
    # Queue of nodes with in_degree 0
    queue = [cid for cid, deg in in_degree.items() if deg == 0]
    order = []
    
    while queue:
        # Sort queue to ensure deterministic ordering
        queue.sort()
        curr = queue.pop(0)
        order.append(curr)
        for neighbor in adj[curr]:
            in_degree[neighbor] -= 1
            if in_degree[neighbor] == 0:
                queue.append(neighbor)
                
    # If there's a cycle, ensure we don't lose any nodes
    if len(order) < len(components):
        remaining = [cid for cid in components if cid not in order]
        order.extend(remaining)
        
    return order


def get_next_actionable_components(registry: SystemRegistry, action_type: str) -> List[str]:
    """Returns a list of component IDs ready for the specified action_type.
    
    action_type must be "plan" or "implement".
    """
    if action_type not in ("plan", "implement"):
        raise ValueError("action_type must be either 'plan' or 'implement'")
        
    topo_order = topological_sort_components(registry.components)
    actionable = []
    
    if action_type == "plan":
        # Ready to plan if current stage is ARCH_APPROVED, and ALL transitive dependencies are at least ARCH_APPROVED (not DECLARED)
        for cid in topo_order:
            comp = registry.components.get(cid)
            if not comp or comp.stage != LifecycleStage.ARCH_APPROVED:
                continue
                
            # Scan transitive dependencies to make sure none are in DECLARED
            visited = set()
            queue = get_component_dependencies(comp)
            has_declared_dep = False
            while queue:
                curr = queue.pop(0)
                if curr in visited:
                    continue
                visited.add(curr)
                dep_comp = registry.components.get(curr)
                if dep_comp:
                    if dep_comp.stage == LifecycleStage.DECLARED:
                        has_declared_dep = True
                        break
                    for dep in get_component_dependencies(dep_comp):
                        if dep not in visited:
                            queue.append(dep)
            
            if not has_declared_dep:
                actionable.append(cid)
                
    elif action_type == "implement":
        # Ready to implement if current stage is PLAN_APPROVED, and ALL direct dependencies are IMPLEMENTED
        for cid in topo_order:
            comp = registry.components.get(cid)
            if not comp or comp.stage != LifecycleStage.PLAN_APPROVED:
                continue
                
            # Direct dependencies check
            direct_deps = get_component_dependencies(comp)
            has_non_implemented_dep = False
            for dep in direct_deps:
                dep_comp = registry.components.get(dep)
                if dep_comp and dep_comp.stage != LifecycleStage.IMPLEMENTED:
                    has_non_implemented_dep = True
                    break
                    
            if not has_non_implemented_dep:
                actionable.append(cid)
                
    return actionable


def cascade_invalidate_component(registry: SystemRegistry, component_id: str) -> List[str]:
    """Runs the cascading invalidation wave starting from a modified component_id.
    
    Returns a list of downgraded/resetted component IDs.
    """
    # Build dependent mapping: cid -> list of cids that directly depend on it
    dependents_map = {cid: [] for cid in registry.components}
    
    # 1. Map schema, implements, parent dependencies
    for cid, comp in registry.components.items():
        deps = get_component_dependencies(comp)
        for dep in deps:
            if dep in dependents_map:
                dependents_map[dep].append(cid)
                
    # 2. Map usage tree dependencies (caller depends on callee)
    def walk_usage_tree(node: UsageNode):
        caller_id = node.caller_id
        callee_id = node.component_id
        if callee_id in dependents_map and caller_id in dependents_map:
            if caller_id not in dependents_map[callee_id]:
                dependents_map[callee_id].append(caller_id)
        for child in node.dependencies:
            walk_usage_tree(child)
            
    for tree in registry.usage_trees.values():
        walk_usage_tree(tree)
        
    # Queue for BFS cascade
    queue = [component_id]
    visited = set()
    downgraded = []
    
    while queue:
        curr_id = queue.pop(0)
        if curr_id in visited:
            continue
        visited.add(curr_id)
        
        comp = registry.components.get(curr_id)
        if not comp:
            continue
            
        # Perform stage resets
        if curr_id == component_id:
            if comp.stage != LifecycleStage.DECLARED:
                comp.stage = LifecycleStage.DECLARED
                downgraded.append(curr_id)
        else:
            # Upstream dependent
            if comp.stage == LifecycleStage.IMPLEMENTED:
                comp.stage = LifecycleStage.PLAN_APPROVED
                downgraded.append(curr_id)
            elif comp.stage == LifecycleStage.PLAN_APPROVED:
                comp.stage = LifecycleStage.ARCH_APPROVED
                downgraded.append(curr_id)
                
        # Add its direct dependents to queue
        for dep in dependents_map.get(curr_id, []):
            if dep not in visited:
                queue.append(dep)
                
    return downgraded


def check_component_compatibility(registry: SystemRegistry, component_id: str) -> List[str]:
    """Runs all static analysis and usage node validations for a single component.
    
    Returns a list of error detail strings.
    """
    errors = []
    
    # 1. Check registry-wide errors
    global_errors = ArchitectureValidator.validate_registry(registry)
    for err in global_errors:
        if err.component_id == component_id or (err.details and component_id in err.details):
            errors.append(f"Registry: {err.details}")
            
    # 2. Check usage trees errors
    for tree_name, root_node in registry.usage_trees.items():
        tree_errors = ArchitectureValidator.validate_usage_node(
            root_node, registry.components, registry.component_types
        )
        for err in tree_errors:
            if err.component_id == component_id or (err.details and component_id in err.details):
                errors.append(f"Tree '{tree_name}': {err.details} (Node: '{err.node_id}')")
                
    return errors


def compile_contract_markdown(comp: Component, registry: SystemRegistry) -> str:
    """Compiles a complete, stateful, and authoritative architectural contract for a component.

    This recursively resolves implemented interfaces, inlines custom type schemas, synthesizes
    invariants, and resolves process-level validation command arrays.
    """
    import json
    # 1. Resolve Interfaces recursively
    resolved_inputs, resolved_outputs = resolve_implements_signature(comp, registry.components)
    resolved_properties = comp.properties

    # 2. Inline Custom Schemas recursively
    try:
        resolved_inputs = resolve_refs(resolved_inputs, registry.components, registry.component_types)
        resolved_outputs = resolve_refs(resolved_outputs, registry.components, registry.component_types)
        resolved_properties = resolve_refs(resolved_properties, registry.components, registry.component_types)
    except Exception as exc:
        return f"Error resolving custom schema references: {str(exc)}"

    # 3. Synthesize Invariants (local and inherited)
    all_invariants = []
    if comp.implementation_spec:
        for inv in comp.implementation_spec.invariants:
            all_invariants.append({
                "name": inv.name,
                "type": inv.type,
                "description": inv.description,
                "source": "Local"
            })

    # Resolve inherited interface chain
    def resolve_interface_chain(component_id: str, visited=None) -> list:
        if visited is None:
            visited = set()
        if component_id in visited:
            return []
        visited.add(component_id)
        c = registry.components.get(component_id)
        if not c:
            return []
        chain = []
        if c.implements_id:
            parent_comp = registry.components.get(c.implements_id)
            if parent_comp:
                chain.append(parent_comp)
                chain.extend(resolve_interface_chain(c.implements_id, visited))
        return chain

    for parent_comp in resolve_interface_chain(comp.id):
        if parent_comp.implementation_spec:
            for inv in parent_comp.implementation_spec.invariants:
                if not any(x["name"] == inv.name for x in all_invariants):
                    all_invariants.append({
                        "name": inv.name,
                        "type": inv.type,
                        "description": inv.description,
                        "source": f"Inherited from '{parent_comp.id}'"
                    })

    # Gathers local logic steps
    logic_steps = []
    if comp.implementation_spec:
        # Sort steps by sequence just in case
        sorted_steps = sorted(comp.implementation_spec.logic_steps, key=lambda x: x.sequence)
        for step in sorted_steps:
            alg_str = f" (Algorithm: {step.algorithm})" if step.algorithm else ""
            comp_str = f" [Complexity: {step.complexity}]" if step.complexity else ""
            logic_steps.append(f"{step.sequence}. **{step.name}**{alg_str}{comp_str}: {step.description}")

    # 4. Format Validations
    validation_commands = []
    if comp.implementation_spec and comp.implementation_spec.validation:
        for ref in comp.implementation_spec.validation:
            tool = registry.validation_tools.get(ref.tool_id)
            if not tool:
                validation_commands.append(
                    f"- **{ref.tool_id}** [ERROR]: Registered definition for tool ID '{ref.tool_id}' not found."
                )
                continue

            base_args = ref.args if ref.args is not None else tool.default_args
            
            # List-based placeholder expansion
            resolved_args = []
            for arg in base_args:
                if arg == "{targets}":
                    resolved_args.extend(ref.targets)
                elif "{targets}" in arg:
                    resolved_args.append(arg.replace("{targets}", ",".join(ref.targets)))
                else:
                    resolved_args.append(arg)

            validation_commands.append(
                f"- **{ref.tool_id}**:\n"
                f"  - Executable: `{tool.executable}`\n"
                f"  - Resolved Arguments: `{resolved_args}`\n"
                f"  - Target Paths: `{ref.targets}`"
            )

    # 5. Format Output Markdown
    doc = []
    doc.append(f"# Component Contract: {comp.id}")
    doc.append(f"**Name**: {comp.name}")
    doc.append(f"**Type**: {comp.type.upper()}")
    doc.append(f"**Status**: {comp.status.upper()}")
    doc.append(f"**Description**: {comp.description}")
    
    if comp.parent_id:
        doc.append(f"**Parent Namespace**: '{comp.parent_id}'")
    if comp.implements_id:
        doc.append(f"**Implements Interface**: '{comp.implements_id}'")
    if comp.location:
        loc_str = f"{comp.location.file_path}"
        if comp.location.line_range:
            loc_str += f"#L{comp.location.line_range[0]}-L{comp.location.line_range[1]}"
        doc.append(f"**Location**: `{loc_str}`")

    doc.append("\n## Properties (State Schema)")
    if resolved_properties:
        doc.append("```json\n" + json.dumps(resolved_properties, indent=2) + "\n```")
    else:
        doc.append("*None defined.*")

    doc.append("\n## Input Contract (Arguments Schema)")
    if resolved_inputs:
        doc.append("```json\n" + json.dumps(resolved_inputs, indent=2) + "\n```")
    else:
        doc.append("*None defined or inherited.*")

    doc.append("\n## Output Contract (Return Schema)")
    if resolved_outputs:
        doc.append("```json\n" + json.dumps(resolved_outputs, indent=2) + "\n```")
    else:
        doc.append("*None defined or inherited.*")

    doc.append("\n## Side Effects")
    if comp.side_effects:
        for se in comp.side_effects:
            doc.append(f"- **{se.target}**: {se.description}")
    else:
        doc.append("*None defined.*")

    doc.append("\n## Invariants & Safety Constraints")
    if all_invariants:
        for inv in all_invariants:
            doc.append(f"- **{inv['name']}** [{inv['type'].upper()}] ({inv['source']}): {inv['description']}")
    else:
        doc.append("*No invariants specified.*")

    doc.append("\n## Sequential Logic Steps")
    if logic_steps:
        for step in logic_steps:
            doc.append(step)
    else:
        doc.append("*No logic steps specified.*")

    doc.append("\n## Planned Modification Tasks")
    if comp.modification_tasks:
        for idx, task_obj in enumerate(comp.modification_tasks, 1):
            status_str = "[COMPLETED]" if task_obj.completed else "[PENDING]"
            doc.append(f"{idx}. {status_str} {task_obj.task}")
    else:
        doc.append("*No modification tasks registered.*")

    doc.append("\n## Resolved Verification Commands")
    if validation_commands:
        doc.extend(validation_commands)
    else:
        doc.append("*No validation commands defined.*")

    return "\n".join(doc)


def run_component_validations(comp: Component, registry: SystemRegistry) -> Tuple[bool, str]:
    """Runs all validation tools configured for the component.
    
    Returns (success, log_output).
    """
    import subprocess
    if not comp.implementation_spec or not comp.implementation_spec.validation:
        return True, "No validation tools defined."
        
    logs = []
    success = True
    
    for ref in comp.implementation_spec.validation:
        tool = registry.validation_tools.get(ref.tool_id)
        if not tool:
            logs.append(f"Tool '{ref.tool_id}': [FAIL] definition not found.")
            success = False
            continue
            
        base_args = ref.args if ref.args is not None else tool.default_args
        
        # Expand placeholders
        resolved_args = []
        for arg in base_args:
            if arg == "{targets}":
                resolved_args.extend(ref.targets)
            elif "{targets}" in arg:
                resolved_args.append(arg.replace("{targets}", ",".join(ref.targets)))
            else:
                resolved_args.append(arg)
                
        cmd = [tool.executable] + resolved_args
        cmd_str = " ".join(cmd)
        logs.append(f"Running command: {cmd_str}")
        
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, check=False)
            logs.append(f"STDOUT:\n{result.stdout}")
            if result.stderr:
                logs.append(f"STDERR:\n{result.stderr}")
            logs.append(f"Exit code: {result.returncode}")
            
            if result.returncode != 0:
                success = False
                logs.append(f"Tool '{ref.tool_id}': [FAIL] returned non-zero exit code.")
            else:
                logs.append(f"Tool '{ref.tool_id}': [PASS]")
        except Exception as exc:
            success = False
            logs.append(f"Tool '{ref.tool_id}': [ERROR] failed to execute: {exc}")
            
    return success, "\n".join(logs)
