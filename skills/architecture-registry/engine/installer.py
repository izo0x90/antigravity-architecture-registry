import sys
import shutil
import json
from pathlib import Path

class SetupManager:
    """Encapsulates the global copy and setup operations for the Architecture Registry
    plugin inside the opencode ecosystem.
    """
    
    def __init__(self, package_root: Path):
        self.package_root = package_root
        self.global_dir = Path.home() / ".config" / "opencode"
        
    def run_setup(self) -> bool:
        """Executes the setup sequence and returns True if successful."""
        print(f"[ArchitectureRegistry] Initiating global setup...")
        print(f"[ArchitectureRegistry] Source package path: {self.package_root}")
        print(f"[ArchitectureRegistry] Target opencode path: {self.global_dir}")
        
        try:
            self._copy_skills()
            self._copy_agents()
            self._configure_mcp_server()
            print("\n[SUCCESS] Global setup completed! Please restart opencode to load.")
            return True
        except Exception as exc:
            print(f"\n[ERROR] Global setup failed: {exc}", file=sys.stderr)
            return False
            
    def _copy_skills(self):
        """Copies the shared skill to the global auto-discovery folder."""
        src_skills = self.package_root / "plugin" / "skills" / "architecture-registry"
        dest_skills = self.global_dir / "skills" / "architecture-registry"
        
        if not src_skills.exists():
            raise FileNotFoundError(f"Source skills folder not found: {src_skills}")
            
        if dest_skills.exists():
            shutil.rmtree(dest_skills)
            
        shutil.copytree(src_skills, dest_skills, dirs_exist_ok=True)
        print(f"[ArchitectureRegistry] Copied skills successfully.")
        
    def _copy_agents(self):
        """Copies the model-agnostic agent prompts to the global agents folder."""
        src_agents = self.package_root / "plugin" / "agents"
        dest_agents = self.global_dir / "agents"
        
        if not src_agents.exists():
            raise FileNotFoundError(f"Source agents folder not found: {src_agents}")
            
        dest_agents.mkdir(parents=True, exist_ok=True)
        for item in src_agents.glob("*.md"):
            shutil.copy2(item, dest_agents / item.name)
            print(f"[ArchitectureRegistry] Copied agent prompt: {item.name}")
            
    def _configure_mcp_server(self):
        """Registers the local server in global opencode.json utilizing 'serve'."""
        global_config_path = self.global_dir / "opencode.json"
        self.global_dir.mkdir(parents=True, exist_ok=True)
        
        config = {}
        if global_config_path.exists():
            try:
                config = json.loads(global_config_path.read_text(encoding="utf-8"))
            except Exception as e:
                print(f"[WARNING] Global config could not be parsed as JSON, overwriting: {e}")
                config = {}
                
        if "$schema" not in config:
            config["$schema"] = "https://opencode.ai/config.json"
            
        config["mcp"] = config.get("mcp", {})
        config["mcp"]["architecture_registry"] = {
            "type": "local",
            "command": ["architecture-registry", "serve"], # Modular subcommand!
            "enabled": True
        }
        
        global_config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")
        print(f"[ArchitectureRegistry] Registered MCP server in global opencode.json")
