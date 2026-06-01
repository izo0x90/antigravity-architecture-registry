from typing import Dict, Any
from .models import JSONSchemaDict
from .exceptions import DSLCompilationError

PrimitivesMap = Dict[str, str]

class DSLCompiler:
    """Compiles language-agnostic simplified type dicts to standard JSON Schema."""

    SUPPORTED_PRIMITIVES: PrimitivesMap = {
        "string": "string",
        "str": "string",
        "text": "string",
        "integer": "integer",
        "int": "integer",
        "number": "number",
        "float": "number",
        "double": "number",
        "boolean": "boolean",
        "bool": "boolean",
    }

    @classmethod
    def compile_type_str(cls, raw_type: str) -> dict:
        """Parses a type string like 'int?', 'string[]', or 'float' into JSON Schema."""
        is_optional = False
        is_array = False
        
        type_str = raw_type.strip()
        
        # 1. Parse Optional suffix
        if type_str.endswith("?"):
            is_optional = True
            type_str = type_str[:-1].strip()
            
        # 2. Parse Array suffix
        if type_str.endswith("[]"):
            is_array = True
            type_str = type_str[:-2].strip()
            
        # 3. Resolve base type
        if type_str not in cls.SUPPORTED_PRIMITIVES:
            if type_str.isidentifier():
                schema: dict = {"type": "object", "title": type_str}
            else:
                raise DSLCompilationError(
                    f"Unsupported primitive type '{type_str}'", raw_type
                )
        else:
            schema = {"type": cls.SUPPORTED_PRIMITIVES[type_str]}
        
        if is_array:
            schema = {
                "type": "array",
                "items": schema
            }
            
        if is_optional:
            schema = {
                "anyOf": [
                    schema,
                    {"type": "null"}
                ]
            }
            
        return schema

    @classmethod
    def compile_shorthand(cls, shorthand: Any) -> JSONSchemaDict:
        """Parses shorthand dictionary or string into standard JSON Schema representation.

        Raises:
            DSLCompilationError: If an unrecognized type or invalid construct is supplied.
        """
        if isinstance(shorthand, str):
            return cls.compile_type_str(shorthand)
            
        if isinstance(shorthand, dict):
            # If it already looks like standard JSON Schema (e.g. has "type"), return as-is
            if "type" in shorthand and isinstance(shorthand["type"], str):
                return shorthand
                
            # Otherwise, it's a shorthand object mapping field names to types
            properties = {}
            required = []
            for field_name, field_val in shorthand.items():
                is_field_optional = False
                if isinstance(field_val, str) and field_val.strip().endswith("?"):
                    is_field_optional = True
                
                compiled_val = cls.compile_shorthand(field_val)
                properties[field_name] = compiled_val
                
                if not is_field_optional:
                    required.append(field_name)
                    
            schema = {
                "type": "object",
                "properties": properties,
            }
            if required:
                schema["required"] = required
            return schema
            
        raise DSLCompilationError(
            f"Invalid shorthand definition type '{type(shorthand).__name__}'", str(shorthand)
        )

