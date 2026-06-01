import unittest
import sys
from pathlib import Path

# Add repository directories to path so imports work correctly and dynamically
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.engine import RegistryEngine
from engine.models import Component, SystemRegistry, SideEffect
from engine.validator import ArchitectureValidator, resolve_refs
from engine.dsl_compiler import DSLCompiler
from engine.exceptions import CompatibilityError

class TestGlobalInvariants(unittest.TestCase):

    def setUp(self):
        # We use a temporary file path
        self.temp_file = Path("temp_test.json")
        if self.temp_file.exists():
            self.temp_file.unlink()

    def tearDown(self):
        if self.temp_file.exists():
            self.temp_file.unlink()

    def test_unregistered_type(self):
        engine = RegistryEngine(self.temp_file)
        engine.seed_component_types() # Seeds valid types
        
        # Manually build SystemRegistry bypassing add_component rules
        comp = Component(
            id="actor_comp",
            name="Actor Component",
            type="invalid_custom_type",
            description="A component with an unregistered type"
        )
        engine.registry.components[comp.id] = comp
        
        errors = ArchitectureValidator.validate_registry(engine.registry)
        self.assertEqual(len(errors), 1, f"Expected 1 error, got {len(errors)}")
        self.assertIn("unregistered type 'invalid_custom_type'", errors[0].details)

    def test_missing_parent(self):
        engine = RegistryEngine(self.temp_file)
        engine.seed_component_types()
        
        comp = Component(
            id="child_comp",
            name="Child Component",
            type="function",
            parent_id="missing_parent_id",
            description="A component pointing to non-existent parent"
        )
        engine.registry.components[comp.id] = comp
        
        errors = ArchitectureValidator.validate_registry(engine.registry)
        self.assertGreaterEqual(len(errors), 1, "Expected at least 1 error")
        self.assertTrue(any("Parent component 'missing_parent_id' referenced by component 'child_comp' does not exist" in e.details for e in errors))

    def test_parenting_cycle(self):
        engine = RegistryEngine(self.temp_file)
        engine.seed_component_types()
        
        # Establish parent cycle: comp_a has parent comp_b, comp_b has parent comp_a
        comp_a = Component(
            id="comp_a",
            name="Component A",
            type="module",
            parent_id="comp_b",
            description="Component A"
        )
        comp_b = Component(
            id="comp_b",
            name="Component B",
            type="module",
            parent_id="comp_a",
            description="Component B"
        )
        engine.registry.components[comp_a.id] = comp_a
        engine.registry.components[comp_b.id] = comp_b
        
        errors = ArchitectureValidator.validate_registry(engine.registry)
        self.assertTrue(any("Cyclic parenting relationship detected" in e.details for e in errors), "Expected parenting cycle error")

    def test_implements_cycle(self):
        engine = RegistryEngine(self.temp_file)
        engine.seed_component_types()
        
        # Establish implements cycle: comp_a implements comp_b, comp_b implements comp_a
        comp_a = Component(
            id="comp_a",
            name="Component A",
            type="interface",
            implements_id="comp_b",
            description="Component A"
        )
        comp_b = Component(
            id="comp_b",
            name="Component B",
            type="interface",
            implements_id="comp_a",
            description="Component B"
        )
        engine.registry.components[comp_a.id] = comp_a
        engine.registry.components[comp_b.id] = comp_b
        
        errors = ArchitectureValidator.validate_registry(engine.registry)
        self.assertTrue(any("Cyclic implements relationship detected" in e.details for e in errors), "Expected implements cycle error")

    def test_contract_mismatches(self):
        engine = RegistryEngine(self.temp_file)
        engine.seed_component_types()
        
        # 1. Define parent/interface DB query contract
        db_interface = Component(
            id="db_interface",
            name="Database Operation Interface",
            type="operation",
            description="Abstract contract",
            inputs=DSLCompiler.compile_shorthand({"query": "string"}),
            outputs=DSLCompiler.compile_shorthand({"rows": "string[]"}),
            side_effects=[SideEffect(target="db", description="DB access")]
        )
        
        # 2. Define concrete component with completely different inputs/outputs/side effects
        concrete_comp = Component(
            id="fetch_users",
            name="Fetch Users Method",
            type="function",
            implements_id="db_interface",
            description="Concrete function",
            inputs=DSLCompiler.compile_shorthand({"limit": "int"}), # Type/property name mismatch
            outputs=DSLCompiler.compile_shorthand({"count": "int"}), # Output schema mismatch
            side_effects=[SideEffect(target="network", description="Network access")] # Mismatched/undeclared side effect
        )
        
        engine.registry.components[db_interface.id] = db_interface
        engine.registry.components[concrete_comp.id] = concrete_comp
        
        errors = ArchitectureValidator.validate_registry(engine.registry)
        
        # Verify we catch inputs mismatch, outputs mismatch, and undeclared side effect!
        self.assertTrue(any("inputs" in e.details for e in errors), "Expected inputs contract error")
        self.assertTrue(any("outputs" in e.details for e in errors), "Expected outputs contract error")
        self.assertTrue(any("Undeclared side-effect target 'network'" in e.details for e in errors), "Expected side-effect contract error")

    def test_schema_reference_cycle(self):
        engine = RegistryEngine(self.temp_file)
        engine.seed_component_types()
        
        # Define circular types:
        # Component 'user_type' (class) has properties referring to 'session_type' (class)
        # Component 'session_type' (class) has properties referring to 'user_type' (class)
        user_type = Component(
            id="user_type",
            name="User Data",
            type="class",
            description="User object definition",
            properties={"type": "object", "properties": {"session": {"title": "session_type"}}}
        )
        session_type = Component(
            id="session_type",
            name="Session Data",
            type="class",
            description="Session object definition",
            properties={"type": "object", "properties": {"user": {"title": "user_type"}}}
        )
        
        engine.registry.components[user_type.id] = user_type
        engine.registry.components[session_type.id] = session_type
        
        # Run validate_registry which will recursively resolve schemas
        errors = ArchitectureValidator.validate_registry(engine.registry)
        
        self.assertTrue(any("Circular reference loop detected" in e.details for e in errors), "Expected schema reference cycle error")

if __name__ == "__main__":
    unittest.main()
