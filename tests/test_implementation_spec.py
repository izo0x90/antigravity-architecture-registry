import unittest
import json
import sys
from pathlib import Path

# Add repository directories to path dynamically
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.models import SystemRegistry, Component, ComponentTypeRule, ImplementationSpec, LogicStep, InvariantRule, InvariantType
from engine.validator import ArchitectureValidator

class TestImplementationSpec(unittest.TestCase):

    def setUp(self):
        # Establish base component types map
        self.component_types = {
            "interface": ComponentTypeRule(allows_properties=True, allows_signature=True),
            "function": ComponentTypeRule(allows_properties=False, allows_signature=True)
        }

    def test_logic_steps_valid_contiguous(self):
        """Verify that logic steps sequentially numbered 1 to N pass validation."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    description="Test component",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        logic_steps=[
                            LogicStep(sequence=1, name="Step 1", description="First"),
                            LogicStep(sequence=2, name="Step 2", description="Second"),
                            LogicStep(sequence=3, name="Step 3", description="Third")
                        ]
                    )
                )
            }
        )
        errors = ArchitectureValidator.validate_registry(registry)
        self.assertEqual(len(errors), 0, f"Expected 0 errors, got: {[e.details for e in errors]}")

    def test_logic_steps_invalid_non_contiguous(self):
        """Verify that logic steps missing step 2 (sequence 1, 3) fails validation."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    description="Test component",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        logic_steps=[
                            LogicStep(sequence=1, name="Step 1", description="First"),
                            LogicStep(sequence=3, name="Step 3", description="Third")
                        ]
                    )
                )
            }
        )
        errors = ArchitectureValidator.validate_registry(registry)
        self.assertTrue(any("logic steps must be sequentially contiguous starting from 1" in e.details for e in errors))

    def test_logic_steps_invalid_duplicates(self):
        """Verify that logic steps with duplicate sequence indices fail validation."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    description="Test component",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        logic_steps=[
                            LogicStep(sequence=1, name="Step 1", description="First"),
                            LogicStep(sequence=1, name="Step 1 duplicate", description="Duplicate index")
                        ]
                    )
                )
            }
        )
        errors = ArchitectureValidator.validate_registry(registry)
        self.assertTrue(any("logic steps must have unique sequence indices" in e.details for e in errors))

    def test_logic_steps_not_starting_from_one(self):
        """Verify that logic steps that start from an index other than 1 fail validation."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "comp1": Component(
                    id="comp1",
                    name="Component 1",
                    type="function",
                    description="Test component",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        logic_steps=[
                            LogicStep(sequence=2, name="Step 2", description="Starts from 2"),
                            LogicStep(sequence=3, name="Step 3", description="Ends at 3")
                        ]
                    )
                )
            }
        )
        errors = ArchitectureValidator.validate_registry(registry)
        self.assertTrue(any("logic steps must be sequentially contiguous starting from 1" in e.details for e in errors))

    def test_invariant_inheritance_type_mismatch(self):
        """Verify that child components overriding parent invariants must preserve their invariant type."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "base_interface": Component(
                    id="base_interface",
                    name="Base Interface",
                    type="interface",
                    description="Abstract interface",
                    status="existing",
                    implementation_spec=ImplementationSpec(
                        invariants=[
                            InvariantRule(name="timeout_check", type=InvariantType.PRE_CONDITION, description="Must not time out.")
                        ]
                    )
                ),
                "concrete_impl": Component(
                    id="concrete_impl",
                    name="Concrete Implementation",
                    type="function",
                    implements_id="base_interface",
                    description="Concrete class method",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        invariants=[
                            InvariantRule(name="timeout_check", type=InvariantType.POST_CONDITION, description="Redefined as post-condition.")
                        ]
                    )
                )
            }
        )
        errors = ArchitectureValidator.validate_registry(registry)
        self.assertTrue(
            any("type mismatch with parent interface" in e.details for e in errors),
            f"Expected type mismatch error, got: {[e.details for e in errors]}"
        )

    def test_invariant_inheritance_type_match(self):
        """Verify that child components overriding parent invariants with same type succeed validation."""
        registry = SystemRegistry(
            component_types=self.component_types,
            components={
                "base_interface": Component(
                    id="base_interface",
                    name="Base Interface",
                    type="interface",
                    description="Abstract interface",
                    status="existing",
                    implementation_spec=ImplementationSpec(
                        invariants=[
                            InvariantRule(name="timeout_check", type=InvariantType.PRE_CONDITION, description="Must not time out.")
                        ]
                    )
                ),
                "concrete_impl": Component(
                    id="concrete_impl",
                    name="Concrete Implementation",
                    type="function",
                    implements_id="base_interface",
                    description="Concrete class method",
                    status="new",
                    implementation_spec=ImplementationSpec(
                        invariants=[
                            # Type matches exactly
                            InvariantRule(name="timeout_check", type=InvariantType.PRE_CONDITION, description="We agree on pre-condition.")
                        ]
                    )
                )
            }
        )
        errors = ArchitectureValidator.validate_registry(registry)
        self.assertEqual(len(errors), 0, f"Expected 0 errors, got: {[e.details for e in errors]}")


if __name__ == "__main__":
    unittest.main()
