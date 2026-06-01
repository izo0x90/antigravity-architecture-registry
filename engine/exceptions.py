class RegistryError(Exception):
    """Base exception for all architecture registry operations."""
    pass


class ComponentNotFoundError(RegistryError):
    """Raised when a requested component does not exist in the registry."""

    def __init__(self, component_id: str):
        super().__init__(
            f"Component with ID '{component_id}' was not found in the registry."
        )
        self.component_id: str = component_id


class DuplicateComponentError(RegistryError):
    """Raised when trying to add a component with an ID that already exists."""

    def __init__(self, component_id: str):
        super().__init__(
            f"Component with ID '{component_id}' already exists in the registry."
        )
        self.component_id: str = component_id


class DSLCompilationError(RegistryError):
    """Raised when compiling shorthand DSL into JSON Schema fails."""

    def __init__(self, message: str, raw_input_str: str):
        super().__init__(
            f"Failed to compile shorthand DSL: {message} (Input: {raw_input_str})"
        )
        self.raw_input_str: str = raw_input_str


class CompatibilityError(RegistryError):
    """Raised when a usage-site node's expectations violate the component's defined interface."""

    def __init__(self, node_id: str, component_id: str, details: str):
        super().__init__(
            f"Compatibility mismatch at usage node '{node_id}' calling '{component_id}': {details}"
        )
        self.node_id: str = node_id
        self.component_id: str = component_id
        self.details: str = details
