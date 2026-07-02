import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';

const sourceDir = process.cwd(); // Root of our installed/cloned package
const globalDir = path.join(os.homedir(), '.config', 'opencode');

console.log(`[ArchitectureRegistry] Copying global assets to: ${globalDir}`);

// Recursive copy helper
function copyRecursive(src, dest) {
  if (!fs.existsSync(src)) return;
  const stats = fs.statSync(src);
  if (stats.isDirectory()) {
    if (!fs.existsSync(dest)) fs.mkdirSync(dest, { recursive: true });
    fs.readdirSync(src).forEach((child) => {
      copyRecursive(path.join(src, child), path.join(dest, child));
    });
  } else {
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.copyFileSync(src, dest);
  }
}

// 1. Copy skills globally
copyRecursive(
  path.join(sourceDir, 'plugin', 'skills', 'architecture-registry'),
  path.join(globalDir, 'skills', 'architecture-registry')
);

// 2. Copy agents globally
copyRecursive(
  path.join(sourceDir, 'plugin', 'agents'),
  path.join(globalDir, 'agents')
);

// 3. Register MCP server in global opencode.json
const globalConfigPath = path.join(globalDir, 'opencode.json');
try {
  fs.mkdirSync(globalDir, { recursive: true });
  let config = {};
  if (fs.existsSync(globalConfigPath)) {
    config = JSON.parse(fs.readFileSync(globalConfigPath, 'utf8'));
  } else {
    config = { "$schema": "https://opencode.ai/config.json" };
  }

  config.mcp = config.mcp || {};
  config.mcp["architecture_registry"] = {
    type: "local",
    command: ["uv", "run", "--project", sourceDir, "architecture-registry"],
    enabled: true
  };

  fs.writeFileSync(globalConfigPath, JSON.stringify(config, null, 2), 'utf8');
  console.log(`[ArchitectureRegistry] Registered MCP server globally!`);
} catch (e) {
  console.error(`[ArchitectureRegistry] Failed to update global opencode.json: ${e.message}`);
}
