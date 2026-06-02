import unittest
import sys
import asyncio
from pathlib import Path

# Add repository directories to path dynamically
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.models import (
    SystemRegistry, Component, ComponentTypeRule, ImplementationSpec,
    LogicStep, InvariantRule, InvariantType, ValidationToolDefinition, ComponentValidationRef
)
from mcp_server import compile_component_contract
from engine.engine import RegistryEngine

class MockSession:
    def __init__(self):
        self.active_engine_instance = None

class MockContext:
    def __init__(self):
        self.session = MockSession()

class TestCompiledContract(unittest.TestCase):

    def setUp(self):
        self.component_types = {
            "interface": ComponentTypeRule(allows_properties=True, allows_signature=True),
            "function": ComponentTypeRule(allows_properties=False, allows_signature=True)
        }

    def run_async(self, coro):
        return asyncio.run(coro)

    def test_global_tool_resolution_and_placeholder_replacement(self):
        """Verify that global tools are resolved and {targets} token replacement works."""
        registry = SystemRegistry(
            component_types=self.component_types,
            validation_tools={
                "ruff": ValidationToolDefinition(
                    id="ruff",
                    executable="ruff",
                    default_args=["check", "{targets}"]
                )
            },
            components={
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    description="Test component",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        validation=[
                            ComponentValidationRef(
                                tool_id="ruff",
                                targets=["src/file1.py", "src/file2.py"]
                            )
                        ]
                    )
                )
            }
        )
        
        # Setup engine and context
        engine = RegistryEngine(Path("fake_path.json"))
        engine.registry = registry
        ctx = MockContext()
        ctx.session.active_engine_instance = engine
        
        contract = self.run_async(compile_component_contract("comp1", ctx=ctx))
        
        # Verify that ruff command was resolved with ruff's executable and replaced targets
        self.assertIn("ruff", contract)
        self.assertIn("['check', 'src/file1.py', 'src/file2.py']", contract)
        self.assertIn("Target Paths: `['src/file1.py', 'src/file2.py']`", contract)

    def test_argument_overrides(self):
        """Verify that component-level args overrides default global args."""
        registry = SystemRegistry(
            component_types=self.component_types,
            validation_tools={
                "ruff": ValidationToolDefinition(
                    id="ruff",
                    executable="ruff",
                    default_args=["check", "{targets}"]
                )
            },
            components={
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    description="Test component",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        validation=[
                            ComponentValidationRef(
                                tool_id="ruff",
                                targets=["src/file1.py"],
                                args=["format", "--diff", "{targets}"]
                            )
                        ]
                    )
                )
            }
        )
        
        engine = RegistryEngine(Path("fake_path.json"))
        engine.registry = registry
        ctx = MockContext()
        ctx.session.active_engine_instance = engine
        
        contract = self.run_async(compile_component_contract("comp1", ctx=ctx))
        
        self.assertIn("ruff", contract)
        # It should completely suppress default_args ['check', '{targets}']
        self.assertNotIn("check", contract)
        # It should resolve component level arg override format --diff
        self.assertIn("['format', '--diff', 'src/file1.py']", contract)

    def test_recursive_interface_and_invariants_resolution(self):
        """Verify recursive resolution of inputs/outputs schemas and local/inherited invariants."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "base_interface": Component(
                    id="base_interface",
                    name="Base Interface",
                    type="interface",
                    description="Abstract interface",
                    status="new",
                    inputs={"type": "object", "properties": {"userId": {"type": "string"}}},
                    outputs={"type": "object", "properties": {"success": {"type": "boolean"}}},
                    implementation_spec=ImplementationSpec(
                        invariants=[
                            InvariantRule(name="inv_base", type=InvariantType.PRE_CONDITION, description="Base pre-condition")
                        ]
                    )
                ),
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    implements_id="base_interface",
                    description="Implements base_interface",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        invariants=[
                            InvariantRule(name="inv_local", type=InvariantType.POST_CONDITION, description="Local post-condition")
                        ]
                    )
                )
            }
        )
        
        engine = RegistryEngine(Path("fake_path.json"))
        engine.registry = registry
        ctx = MockContext()
        ctx.session.active_engine_instance = engine
        
        contract = self.run_async(compile_component_contract("comp1", ctx=ctx))
        
        # Verify inputs and outputs are recursively inherited from base_interface
        self.assertIn("userId", contract)
        self.assertIn("success", contract)
        
        # Verify local and inherited invariants are synthesized
        self.assertIn("inv_base", contract)
        self.assertIn("inv_local", contract)
        self.assertIn("Inherited from 'base_interface'", contract)
        self.assertIn("Local", contract)


if __name__ == "__main__":
    unittest.main()
