import unittest
import sys
import os
from pathlib import Path

# Add repository directories to path so imports work correctly and dynamically
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.engine import RegistryEngine
from engine.models import (
    Component,
    SystemRegistry,
    LifecycleStage,
    ImplementationSpec,
    LogicStep,
    ComponentValidationRef,
    ValidationToolDefinition,
    ComponentTypeRule,
)
from engine.validator import (
    check_component_compatibility,
    cascade_invalidate_component,
    run_component_validations,
    get_component_dependencies,
)


class TestLifecycleAndInvalidation(unittest.TestCase):

    def setUp(self):
        self.temp_file = Path("temp_lifecycle_test.json")
        if self.temp_file.exists():
            self.temp_file.unlink()
        self.engine = RegistryEngine(self.temp_file)
        self.engine.seed_component_types()

    def tearDown(self):
        if self.temp_file.exists():
            self.temp_file.unlink()

    def test_lifecycle_stage_transitions_success(self):
        """Verify the progressive advancement across declared -> arch_approved -> plan_approved -> implemented."""
        # 1. Register a new component (default is DECLARED)
        comp = Component(
            id="auth_service",
            name="Auth Service",
            type="function",
            description="Performs user authentication",
            status="new",
            inputs={"username": "str", "password": "str"},
            outputs={"token": "str"},
        )
        self.engine.add_component(comp)
        self.engine.save()

        # Reload to verify persistence
        self.engine.load()
        loaded_comp = self.engine.registry.components["auth_service"]
        self.assertEqual(loaded_comp.stage, LifecycleStage.DECLARED)

        # 2. Transition: DECLARED -> ARCH_APPROVED
        # Run programmatic gate
        errors = check_component_compatibility(self.engine.registry, "auth_service")
        self.assertEqual(len(errors), 0)
        
        # Advance stage
        loaded_comp.stage = LifecycleStage.ARCH_APPROVED
        self.engine.save()

        # 3. Transition: ARCH_APPROVED -> PLAN_APPROVED
        # Give it a valid implementation spec with sequential logic steps
        loaded_comp.implementation_spec = ImplementationSpec(
            logic_steps=[
                LogicStep(sequence=1, name="Verify Password", description="Hash and compare input password"),
                LogicStep(sequence=2, name="Issue Token", description="Generate JWT and sign with key"),
            ]
        )
        # Run check
        errors = check_component_compatibility(self.engine.registry, "auth_service")
        self.assertEqual(len(errors), 0)

        # Advance stage
        loaded_comp.stage = LifecycleStage.PLAN_APPROVED
        self.engine.save()

        # 4. Transition: PLAN_APPROVED -> IMPLEMENTED
        # Register mock validation tool
        python_exec = sys.executable
        mock_tool = ValidationToolDefinition(
            id="mock_linter",
            executable=python_exec,
            default_args=["-c", "import sys; sys.exit(0)"],
        )
        self.engine.registry.validation_tools["mock_linter"] = mock_tool
        
        # Reference validation tool in implementation spec
        loaded_comp.implementation_spec.validation = [
            ComponentValidationRef(tool_id="mock_linter", targets=["auth_service.py"])
        ]
        self.engine.save()

        # Run validations
        success, logs = run_component_validations(loaded_comp, self.engine.registry)
        self.assertTrue(success)
        self.assertIn("Exit code: 0", logs)

        # Advance stage
        loaded_comp.stage = LifecycleStage.IMPLEMENTED
        self.engine.save()

        # Reload and assert stage
        self.engine.load()
        final_comp = self.engine.registry.components["auth_service"]
        self.assertEqual(final_comp.stage, LifecycleStage.IMPLEMENTED)

    def test_lifecycle_topological_guardrails_plan_blocked(self):
        """Verify that a component cannot be planned if its dependencies are still in the DECLARED stage."""
        # Main module to parent our functions
        comp_m = Component(
            id="app_module",
            name="App Module",
            type="module",
            description="Main namespace",
            status="new",
            stage=LifecycleStage.IMPLEMENTED,
        )
        self.engine.add_component(comp_m)

        # A depends on B. B is in DECLARED.
        comp_b = Component(
            id="db_read",
            name="DB Read",
            type="function",
            parent_id="app_module",
            description="Reads from database",
            status="new",
            inputs={"query": "str"},
            outputs={"result": "str"},
        )
        # A references B in inputs schema title to establish a dependency link
        comp_a = Component(
            id="user_service",
            name="User Service",
            type="function",
            parent_id="app_module",
            description="Handles user profiles",
            status="new",
            inputs={"db_ref": {"type": "object", "title": "db_read"}},
            outputs={"profile": "str"},
        )
        self.engine.add_component(comp_b)
        self.engine.add_component(comp_a)
        
        # Verify dependency link
        self.assertIn("db_read", get_component_dependencies(comp_a))

        # A is in ARCH_APPROVED, but B is still in DECLARED
        comp_a.stage = LifecycleStage.ARCH_APPROVED
        
        # Emulate the planning check
        visited = set()
        queue = get_component_dependencies(comp_a)
        declared_deps = []
        while queue:
            curr = queue.pop(0)
            if curr in visited:
                continue
            visited.add(curr)
            dep_comp = self.engine.registry.components.get(curr)
            if dep_comp:
                if dep_comp.stage == LifecycleStage.DECLARED:
                    declared_deps.append(curr)
                for dep in get_component_dependencies(dep_comp):
                    if dep not in visited:
                        queue.append(dep)

        # It must correctly block planning because db_read is in DECLARED
        self.assertEqual(declared_deps, ["db_read"])

    def test_lifecycle_topological_guardrails_implementation_blocked(self):
        """Verify that a component cannot proceed to IMPLEMENTED if its direct dependencies are not IMPLEMENTED."""
        comp_m = Component(
            id="app_module",
            name="App Module",
            type="module",
            description="Main namespace",
            status="new",
            stage=LifecycleStage.IMPLEMENTED,
        )
        self.engine.add_component(comp_m)

        # A depends on B. B is in PLAN_APPROVED.
        comp_b = Component(
            id="db_read",
            name="DB Read",
            type="function",
            parent_id="app_module",
            description="Reads from database",
            status="new",
            stage=LifecycleStage.PLAN_APPROVED,
        )
        comp_a = Component(
            id="user_service",
            name="User Service",
            type="function",
            parent_id="app_module",
            description="Handles user profiles",
            status="new",
            stage=LifecycleStage.PLAN_APPROVED,
            inputs={"db_ref": {"type": "object", "title": "db_read"}},
        )
        self.engine.add_component(comp_b)
        self.engine.add_component(comp_a)

        # Emulate implementation check bottom-up
        direct_deps = get_component_dependencies(comp_a)
        non_implemented_deps = []
        for dep in direct_deps:
            dep_comp = self.engine.registry.components.get(dep)
            if dep_comp and dep_comp.stage != LifecycleStage.IMPLEMENTED:
                non_implemented_deps.append((dep, dep_comp.stage))

        # Must correctly identify db_read is not implemented
        self.assertEqual(len(non_implemented_deps), 1)
        self.assertEqual(non_implemented_deps[0][0], "db_read")
        self.assertEqual(non_implemented_deps[0][1], LifecycleStage.PLAN_APPROVED)

    def test_cascade_invalidation_waves(self):
        """Verify BFS cascade invalidation transitions dependent layers bottom-up when interfaces update."""
        comp_m = Component(
            id="app_module",
            name="App Module",
            type="module",
            description="Main namespace",
            status="new",
        )
        self.engine.add_component(comp_m)

        # Create a hierarchy chain: C -> B -> A (A depends on B, B depends on C)
        comp_c = Component(
            id="db_driver",
            name="DB Driver",
            type="function",
            parent_id="app_module",
            description="Low level DB driver",
            status="existing",
            stage=LifecycleStage.IMPLEMENTED,
            inputs={"conn_str": "str"},
            outputs={"conn": "str"},
        )
        comp_b = Component(
            id="db_read",
            name="DB Read",
            type="function",
            parent_id="app_module",
            description="Database reader function",
            status="existing",
            stage=LifecycleStage.IMPLEMENTED,
            inputs={"query": "str", "driver_ref": {"type": "object", "title": "db_driver"}},
            outputs={"res": "str"},
        )
        comp_a = Component(
            id="user_service",
            name="User Service",
            type="function",
            parent_id="app_module",
            description="High level user profile service",
            status="existing",
            stage=LifecycleStage.IMPLEMENTED,
            inputs={"user_id": "str", "reader_ref": {"type": "object", "title": "db_read"}},
            outputs={"user": "str"},
        )
        
        self.engine.add_component(comp_c)
        self.engine.add_component(comp_b)
        self.engine.add_component(comp_a)
        self.engine.save()

        # Verify initial stages
        self.assertEqual(self.engine.registry.components["db_driver"].stage, LifecycleStage.IMPLEMENTED)
        self.assertEqual(self.engine.registry.components["db_read"].stage, LifecycleStage.IMPLEMENTED)
        self.assertEqual(self.engine.registry.components["user_service"].stage, LifecycleStage.IMPLEMENTED)

        # Trigger update of C's signature (e.g. changing input keys)
        updates = {"inputs": {"conn_string": "str"}}
        downgraded = self.engine.update_component("db_driver", updates)

        # C is directly modified -> stage reset to DECLARED
        # B is direct dependent of C -> stage downgraded from IMPLEMENTED to PLAN_APPROVED
        # A is downstream dependent of B -> stage downgraded from IMPLEMENTED to PLAN_APPROVED
        self.assertEqual(self.engine.registry.components["db_driver"].stage, LifecycleStage.DECLARED)
        self.assertEqual(self.engine.registry.components["db_read"].stage, LifecycleStage.PLAN_APPROVED)
        self.assertEqual(self.engine.registry.components["user_service"].stage, LifecycleStage.PLAN_APPROVED)

        # Assert downgraded list returned
        self.assertIn("db_driver", downgraded)
        self.assertIn("db_read", downgraded)
        self.assertIn("user_service", downgraded)

        # Perform another wave from PLAN_APPROVED state
        # If we update B's signature now, B becomes DECLARED, and A (which is PLAN_APPROVED) downgrades to ARCH_APPROVED
        updates_b = {"inputs": {"sql_query": "str"}}
        downgraded_b = self.engine.update_component("db_read", updates_b)

        self.assertEqual(self.engine.registry.components["db_read"].stage, LifecycleStage.DECLARED)
        self.assertEqual(self.engine.registry.components["user_service"].stage, LifecycleStage.ARCH_APPROVED)
        
        self.assertIn("db_read", downgraded_b)
        self.assertIn("user_service", downgraded_b)

    def test_get_next_actionable_components(self):
        """Verify get_next_actionable_components returns correct topological ready-lists."""
        from engine.validator import get_next_actionable_components
        
        comp_m = Component(
            id="app_module",
            name="App Module",
            type="module",
            description="Main namespace",
            status="new",
            stage=LifecycleStage.IMPLEMENTED,
        )
        self.engine.add_component(comp_m)

        # C is ARCH_APPROVED, B is DECLARED and depends on C, A is DECLARED and depends on B
        comp_c = Component(
            id="db_driver",
            name="DB Driver",
            type="function",
            parent_id="app_module",
            description="Low level DB driver",
            status="new",
            stage=LifecycleStage.ARCH_APPROVED,
        )
        comp_b = Component(
            id="db_read",
            name="DB Read",
            type="function",
            parent_id="app_module",
            description="Database reader function",
            status="new",
            stage=LifecycleStage.DECLARED,
            inputs={"driver_ref": {"type": "object", "title": "db_driver"}},
        )
        comp_a = Component(
            id="user_service",
            name="User Service",
            type="function",
            parent_id="app_module",
            description="High level user profile service",
            status="new",
            stage=LifecycleStage.DECLARED,
            inputs={"reader_ref": {"type": "object", "title": "db_read"}},
        )
        self.engine.add_component(comp_c)
        self.engine.add_component(comp_b)
        self.engine.add_component(comp_a)
        self.engine.save()

        # Phase 1: Plan ready-list
        # Under action_type="plan":
        # - db_driver is ARCH_APPROVED and has no dependencies (or only app_module which is IMPLEMENTED). Ready to plan.
        # - db_read is DECLARED. NOT ready to plan.
        # - user_service is DECLARED. NOT ready to plan.
        ready_plan = get_next_actionable_components(self.engine.registry, "plan")
        self.assertEqual(ready_plan, ["db_driver"])

        # Let's advance db_driver to PLAN_APPROVED (which is > ARCH_APPROVED) and promote db_read to ARCH_APPROVED
        comp_c.stage = LifecycleStage.PLAN_APPROVED
        comp_b.stage = LifecycleStage.ARCH_APPROVED
        self.engine.save()

        # Now, db_read's stage is ARCH_APPROVED and its dependencies are all >= ARCH_APPROVED. db_read should be ready to plan.
        ready_plan = get_next_actionable_components(self.engine.registry, "plan")
        self.assertEqual(sorted(ready_plan), sorted(["db_read"]))

        # Phase 2: Implement ready-list
        # Under action_type="implement":
        # - Components must be in PLAN_APPROVED stage, and all direct dependencies must be IMPLEMENTED.
        # Let's set db_driver and db_read to PLAN_APPROVED.
        comp_c.stage = LifecycleStage.PLAN_APPROVED
        comp_b.stage = LifecycleStage.PLAN_APPROVED
        self.engine.save()

        # Neither is ready to implement because db_driver's dependency (app_module) is IMPLEMENTED, but db_driver itself is PLAN_APPROVED (so it can be implemented).
        # db_read is PLAN_APPROVED but its dependency db_driver is NOT IMPLEMENTED.
        # So only db_driver is ready to implement!
        ready_impl = get_next_actionable_components(self.engine.registry, "implement")
        self.assertEqual(ready_impl, ["db_driver"])

        # Let's implement db_driver.
        comp_c.stage = LifecycleStage.IMPLEMENTED
        self.engine.save()

        # Now db_read's dependency db_driver is IMPLEMENTED, and db_read is PLAN_APPROVED. db_read should be ready to implement.
        ready_impl = get_next_actionable_components(self.engine.registry, "implement")
        self.assertEqual(ready_impl, ["db_read"])

    def test_cli_update_component(self):
        """Verify cli update-component updates fields and handles cascading invalidation."""
        from cli import main
        
        comp_m = Component(
            id="app_module",
            name="App Module",
            type="module",
            description="Main namespace",
            status="new",
            stage=LifecycleStage.IMPLEMENTED,
        )
        self.engine.add_component(comp_m)

        comp_b = Component(
            id="db_read",
            name="DB Read",
            type="function",
            parent_id="app_module",
            description="Database reader function",
            status="existing",
            stage=LifecycleStage.IMPLEMENTED,
            inputs={"query": "str"},
        )
        comp_a = Component(
            id="user_service",
            name="User Service",
            type="function",
            parent_id="app_module",
            description="High level user profile service",
            status="existing",
            stage=LifecycleStage.IMPLEMENTED,
            inputs={"reader_ref": {"type": "object", "title": "db_read"}},
        )
        self.engine.add_component(comp_b)
        self.engine.add_component(comp_a)
        self.engine.save()

        # Call update-component CLI with name and type update (doesn't trigger invalidation since signatures didn't change)
        ret = main([
            "--file", str(self.temp_file),
            "update-component",
            "--id", "db_read",
            "--name", "New DB Read Name",
            "--type", "function",
        ])
        self.assertEqual(ret, 0)
        
        # Reload and check
        self.engine.load()
        self.assertEqual(self.engine.registry.components["db_read"].name, "New DB Read Name")
        self.assertEqual(self.engine.registry.components["db_read"].stage, LifecycleStage.IMPLEMENTED)
        self.assertEqual(self.engine.registry.components["user_service"].stage, LifecycleStage.IMPLEMENTED)

        # Call update-component with signature change (inputs-dsl/inputs)
        ret = main([
            "--file", str(self.temp_file),
            "update-component",
            "--id", "db_read",
            "--inputs", "sql_query:str",
        ])
        self.assertEqual(ret, 0)

        # Reload and check cascading invalidation: db_read becomes DECLARED, user_service (dependent) downgrades to PLAN_APPROVED
        self.engine.load()
        self.assertEqual(self.engine.registry.components["db_read"].stage, LifecycleStage.DECLARED)
        self.assertEqual(self.engine.registry.components["user_service"].stage, LifecycleStage.PLAN_APPROVED)


if __name__ == "__main__":
    unittest.main()
