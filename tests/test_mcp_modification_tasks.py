import unittest
import sys
import json
from pathlib import Path
from unittest.mock import MagicMock

# Ensure repository directories are in sys.path
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.engine import RegistryEngine
from engine.models import Component, ModificationTask
from mcp_server import update_component


class TestMCPModificationTasks(unittest.IsolatedAsyncioTestCase):

    def setUp(self):
        self.temp_file = Path("temp_test_mcp_tasks.json")
        if self.temp_file.exists():
            self.temp_file.unlink()

        # Setup standard initial registry
        self.engine = RegistryEngine(self.temp_file)
        self.engine.seed_component_types()

        # Add a component for testing
        self.engine.add_component(Component(
            id="math_validator_validate",
            name="validate",
            type="function",
            description="Validates equations"
        ))
        self.engine.save()

        # Create mock Context
        self.ctx = MagicMock()
        self.ctx.session = MagicMock()
        self.ctx.session.active_engine_instance = self.engine

    def tearDown(self):
        if self.temp_file.exists():
            self.temp_file.unlink()

    async def test_update_component_modification_tasks_list_of_strings(self):
        # Update component with modification_tasks passed as a list of strings
        tasks = ["Create helper.py", "Add math tests"]
        result = await update_component(
            id="math_validator_validate",
            modification_tasks=tasks,
            ctx=self.ctx
        )
        self.assertIn("updated successfully", result)

        # Reload from engine and verify
        self.engine.load()
        comp = self.engine.registry.components["math_validator_validate"]
        self.assertEqual(len(comp.modification_tasks), 2)
        self.assertEqual(comp.modification_tasks[0].task, "Create helper.py")
        self.assertFalse(comp.modification_tasks[0].completed)
        self.assertEqual(comp.modification_tasks[1].task, "Add math tests")
        self.assertFalse(comp.modification_tasks[1].completed)

    async def test_update_component_modification_tasks_json_string(self):
        # Update component with modification_tasks passed as a JSON string containing dicts and strings
        tasks_json = json.dumps([
            "Prepare environment",
            {"task": "Write test cases", "completed": True}
        ])
        result = await update_component(
            id="math_validator_validate",
            modification_tasks=tasks_json,
            ctx=self.ctx
        )
        self.assertIn("updated successfully", result)

        # Reload from engine and verify
        self.engine.load()
        comp = self.engine.registry.components["math_validator_validate"]
        self.assertEqual(len(comp.modification_tasks), 2)
        self.assertEqual(comp.modification_tasks[0].task, "Prepare environment")
        self.assertFalse(comp.modification_tasks[0].completed)
        self.assertEqual(comp.modification_tasks[1].task, "Write test cases")
        self.assertTrue(comp.modification_tasks[1].completed)

    async def test_update_component_modification_tasks_invalid_json(self):
        # Pass broken JSON string
        result = await update_component(
            id="math_validator_validate",
            modification_tasks="{invalid json",
            ctx=self.ctx
        )
        self.assertIn("Error parsing/validating modification tasks", result)

    async def test_update_component_modification_tasks_invalid_type(self):
        # Pass list with invalid item type (e.g., integer)
        result = await update_component(
            id="math_validator_validate",
            modification_tasks=["Valid task", 12345],
            ctx=self.ctx
        )
        self.assertIn("Error parsing/validating modification tasks", result)
        self.assertIn("Invalid item type in modification tasks list", result)

    async def test_implement_component_autocompletes_tasks(self):
        # 1. Prepare component in PLAN_APPROVED stage with uncompleted tasks
        from engine.models import LifecycleStage, ModificationTask
        from mcp_server import implement_component

        comp = self.engine.registry.components["math_validator_validate"]
        comp.stage = LifecycleStage.PLAN_APPROVED
        comp.modification_tasks = [
            ModificationTask(task="Code implementation", completed=False),
            ModificationTask(task="Unit tests", completed=False)
        ]
        self.engine.save()

        # 2. Call implement_component
        result = await implement_component(
            id="math_validator_validate",
            ctx=self.ctx
        )
        self.assertIn("implementation verified", result)
        self.assertIn("all planned tasks marked as completed", result)

        # 3. Reload and verify component stage and task completion
        self.engine.load()
        updated_comp = self.engine.registry.components["math_validator_validate"]
        self.assertEqual(updated_comp.stage, LifecycleStage.IMPLEMENTED)
        self.assertEqual(len(updated_comp.modification_tasks), 2)
        self.assertTrue(updated_comp.modification_tasks[0].completed)
        self.assertTrue(updated_comp.modification_tasks[1].completed)


if __name__ == "__main__":
    unittest.main()
