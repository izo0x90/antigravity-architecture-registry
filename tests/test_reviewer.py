import unittest
import sys
import json
import shutil
from pathlib import Path

# Add repository directories to path so imports work correctly and dynamically
test_dir = Path(__file__).resolve().parent
repo_root = test_dir.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

from engine.engine import RegistryEngine
from engine.models import (
    Component,
    UsageNode,
    SideEffect,
    ImplementationSpec,
    InvariantRule,
    LogicStep,
    ComponentValidationRef,
)
from engine.visualizer import Visualizer
from cli import main as cli_main


class TestReviewerAndArtifactExport(unittest.TestCase):

    def setUp(self):
        self.temp_file = Path("temp_reviewer_test.json")
        if self.temp_file.exists():
            self.temp_file.unlink()
            
        self.temp_artifact_dir = Path("temp_artifact_review_dir")
        if self.temp_artifact_dir.exists():
            shutil.rmtree(self.temp_artifact_dir)
            
        self.engine = RegistryEngine(self.temp_file)
        self.engine.seed_component_types()
        self.engine.registry.component_types["class"].allows_signature = True


    def tearDown(self):
        if self.temp_file.exists():
            self.temp_file.unlink()
        if self.temp_artifact_dir.exists():
            shutil.rmtree(self.temp_artifact_dir)

    def test_generate_review_markdown_and_export_cli(self):
        """Verify compiling a detailed review markdown and exporting it with CLI arguments."""
        # 1. Register components with rich contracts and implementation specifications
        user_db = Component(
            id="user_db",
            name="User Database Service",
            type="class",
            description="Accesses SQL database for user profiles.",
            status="existing",
            inputs={"queryId": "str", "params": "dict?"},
            outputs={"userRecord": "dict", "found": "bool"},
            side_effects=[
                SideEffect(target="db:read", description="Reads record matching profile query.")
            ],
            properties={"connection_uri": "string", "pool_size": "int"}
        )
        
        crypto_service = Component(
            id="crypto_service",
            name="Cryptography Service",
            type="function",
            description="Handles password checks",
            status="modifying",
            inputs={"password": "str", "hash": "str"},
            outputs={"is_match": "bool"},
            implementation_spec=ImplementationSpec(
                pattern_or_system="Bcrypt Hashing Model",
                invariants=[
                    InvariantRule(name="INV-1", type="state_invariant", description="Never log plainText password payload.")
                ],
                logic_steps=[
                    LogicStep(sequence=1, name="Verify Format", description="Check hash matches standard crypt structure."),
                    LogicStep(sequence=2, name="Run Comparison", description="Bcrypt hash comparison of parameters.")
                ],
                validation=[
                    ComponentValidationRef(tool_id="ruff", targets=["security_module/"])
                ]
            )
        )
        
        self.engine.add_component(user_db)
        self.engine.add_component(crypto_service)
        
        # 2. Setup a usage tree
        root_node = UsageNode(
            node_id="login_call",
            caller_id="login_api",
            component_id="crypto_service",
            description="Verify login passwords.",
            expected_inputs={"password": "str", "hash": "str"},
            expected_outputs={"is_match": "bool"}
        )
        # Add a sub call node
        child_node = UsageNode(
            node_id="db_fetch",
            caller_id="crypto_service",
            component_id="user_db",
            description="Fetch user record matching profile.",
            expected_inputs={"queryId": "str", "params": "dict?"},
            expected_outputs={"userRecord": "dict", "found": "bool"}
        )
        root_node.dependencies.append(child_node)
        
        self.engine.registry.usage_trees["auth_flow"] = root_node
        self.engine.save()

        # 3. Directly verify the Visualizer markdown generator output
        rendered_lines = Visualizer.render_review_markdown(
            "auth_flow", root_node, self.engine.registry.components, self.engine.registry.component_types
        )
        markdown_text = "\n".join(rendered_lines)
        
        # Check for expected visual blocks
        self.assertIn("# 📋 Architecture & Workflow Review: auth_flow", markdown_text)
        self.assertIn("## 1. Executive Summary", markdown_text)
        self.assertIn("## 2. Workflow Visualization", markdown_text)
        self.assertIn("## 3. Referenced Component Contracts", markdown_text)
        
        # Check that individual component details are present
        self.assertIn("Component: `crypto_service`", markdown_text)
        self.assertIn("Component: `user_db`", markdown_text)
        
        # Verify step-by-step logic, side effects, properties, and invariants are captured
        self.assertIn("Bcrypt Hashing Model", markdown_text)
        self.assertIn("[INV-1]", markdown_text)
        self.assertIn("Never log plainText password payload.", markdown_text)
        self.assertIn("1. **Verify Format**", markdown_text)
        self.assertIn("db:read", markdown_text)
        self.assertIn("connection_uri", markdown_text)
        
        # Check that feedback cards and checklist items are included
        self.assertIn("- [ ] Approved", markdown_text)
        self.assertIn("- [ ] Request Changes", markdown_text)
        
        # 4. Run CLI Command to write both review artifact and metadata sidecar
        argv = [
            "-f", str(self.temp_file),
            "visualize",
            "--tree", "auth_flow",
            "--format", "review_markdown",
            "--to-artifact-dir", str(self.temp_artifact_dir)
        ]
        exit_code = cli_main(argv)
        self.assertEqual(exit_code, 0)
        
        # 5. Verify physical file creation and metadata structure
        md_file = self.temp_artifact_dir / "auth_flow_review.md"
        meta_file = self.temp_artifact_dir / "auth_flow_review.md.metadata.json"
        
        self.assertTrue(md_file.exists())
        self.assertTrue(meta_file.exists())
        
        # Verify metadata properties
        meta_content = json.loads(meta_file.read_text(encoding="utf-8"))
        self.assertEqual(meta_content["artifactType"], "ARTIFACT_TYPE_OTHER")
        self.assertEqual(meta_content["requestFeedback"], True)
        self.assertIn("auth_flow", meta_content["summary"])


if __name__ == "__main__":
    unittest.main()
