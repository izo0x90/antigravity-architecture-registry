import unittest
import sys
import json
from pathlib import Path

# Ensure repository directories are in sys.path
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.engine import RegistryEngine
from engine.models import Component, UsageNode, SideEffect
from engine.exceptions import RegistryError, ComponentNotFoundError
from cli import main


class TestUsageNodeCRUD(unittest.TestCase):

    def setUp(self):
        self.temp_file = Path("temp_test_usage_crud.json")
        if self.temp_file.exists():
            self.temp_file.unlink()

        # Setup standard initial registry
        self.engine = RegistryEngine(self.temp_file)
        self.engine.seed_component_types()

        # Add a component that can initiate and be target of calls
        self.engine.add_component(Component(
            id="auth_service",
            name="Auth Service",
            type="class",
            description="Service for authentication"
        ))
        self.engine.add_component(Component(
            id="user_db",
            name="User Database",
            type="class",
            description="Stores user details"
        ))

        # Add a usage tree root
        self.root_node = UsageNode(
            node_id="auth_service_calls_db",
            caller_id="auth_service",
            component_id="user_db",
            description="Query user record",
            dependencies=[]
        )
        self.engine.add_usage_node("auth_flow", None, self.root_node)
        self.engine.save()

    def tearDown(self):
        if self.temp_file.exists():
            self.temp_file.unlink()

    def test_engine_update_usage_node_success(self):
        # Update description, inputs, and side effects
        updates = {
            "description": "Query user record securely",
            "expected_inputs": {"type": "object", "properties": {"userId": {"type": "string"}}},
            "expected_side_effects": [SideEffect(target="db", description="read query")]
        }

        self.engine.update_usage_node("auth_flow", "auth_service_calls_db", updates)
        self.engine.save()

        # Reload engine and verify updates
        new_engine = RegistryEngine(self.temp_file)
        new_engine.load()
        updated_node = new_engine.registry.usage_trees["auth_flow"]

        self.assertEqual(updated_node.description, "Query user record securely")
        self.assertEqual(updated_node.expected_inputs["properties"]["userId"]["type"], "string")
        self.assertEqual(len(updated_node.expected_side_effects), 1)
        self.assertEqual(updated_node.expected_side_effects[0].target, "db")

    def test_engine_update_usage_node_not_found(self):
        with self.assertRaises(RegistryError) as context:
            self.engine.update_usage_node("auth_flow", "missing_node", {"description": "test"})
        self.assertIn("not found in tree", str(context.exception))

    def test_engine_update_usage_node_invalid_component(self):
        # Try updating component_id to one that doesn't exist
        with self.assertRaises(ComponentNotFoundError):
            self.engine.update_usage_node(
                "auth_flow", 
                "auth_service_calls_db", 
                {"component_id": "non_existent_comp"}
            )

    def test_engine_delete_usage_node_child(self):
        # Add a child node
        child = UsageNode(
            node_id="db_calls_audit",
            caller_id="user_db",
            component_id="auth_service",
            description="Audit database read"
        )
        self.engine.add_usage_node("auth_flow", "auth_service_calls_db", child)
        self.engine.save()

        # Verify child is present
        self.assertEqual(len(self.engine.registry.usage_trees["auth_flow"].dependencies), 1)

        # Delete the child
        self.engine.delete_usage_node("auth_flow", "db_calls_audit")
        self.engine.save()

        # Verify child is gone
        new_engine = RegistryEngine(self.temp_file)
        new_engine.load()
        self.assertEqual(len(new_engine.registry.usage_trees["auth_flow"].dependencies), 0)

    def test_engine_delete_usage_node_root(self):
        # Delete root node deletes the entire tree
        self.engine.delete_usage_node("auth_flow", "auth_service_calls_db")
        self.engine.save()

        new_engine = RegistryEngine(self.temp_file)
        new_engine.load()
        self.assertNotIn("auth_flow", new_engine.registry.usage_trees)

    def test_cli_update_node_success(self):
        # Execute update-node CLI command
        status = main([
            "-f", str(self.temp_file),
            "update-node",
            "--tree", "auth_flow",
            "--node-id", "auth_service_calls_db",
            "--description", "Updated from CLI",
            "--expected-inputs-dsl", "userId: str, token: str?",
            "--expected-side-effects", "db:read, log:write"
        ])
        self.assertEqual(status, 0)

        # Verify edits persisted
        new_engine = RegistryEngine(self.temp_file)
        new_engine.load()
        node = new_engine.registry.usage_trees["auth_flow"]
        self.assertEqual(node.description, "Updated from CLI")
        self.assertEqual(node.expected_inputs["properties"]["userId"]["type"], "string")
        self.assertEqual(len(node.expected_side_effects), 2)

    def test_cli_update_node_invalid_inputs_dsl(self):
        # Passing malformed input should trigger failure exit code
        status = main([
            "-f", str(self.temp_file),
            "update-node",
            "--tree", "auth_flow",
            "--node-id", "auth_service_calls_db",
            "--expected-inputs-dsl", "userId: invalid!!type"
        ])
        self.assertEqual(status, 1)

    def test_cli_delete_node_success(self):
        # Execute delete-node CLI command
        status = main([
            "-f", str(self.temp_file),
            "delete-node",
            "--tree", "auth_flow",
            "--node-id", "auth_service_calls_db"
        ])
        self.assertEqual(status, 0)

        # Verify node deleted
        new_engine = RegistryEngine(self.temp_file)
        new_engine.load()
        self.assertNotIn("auth_flow", new_engine.registry.usage_trees)


if __name__ == "__main__":
    unittest.main()
