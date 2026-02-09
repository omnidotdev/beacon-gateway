# Beacon Gateway Development

load("ext://dotenv", "dotenv")

# Load environment from metarepo
env_file = "../../.env.local"
if os.path.exists(env_file):
    dotenv(fn=env_file)

project_name = "beacon-gateway"

# Build the gateway
local_resource(
    "build-%s" % project_name,
    cmd="cargo build",
    deps=["src", "Cargo.toml"],
    labels=[project_name],
)

# Run the gateway (depends on Synapse for LLM routing when available)
gateway_deps = ["build-%s" % project_name]

# Check if Synapse is available (resource registered by parent Tiltfile)
synapse_path = "%s/projects/omni/synapse" % os.environ["HOME"]
if os.path.exists(synapse_path):
    gateway_deps.append("dev-synapse")

local_resource(
    "dev-%s" % project_name,
    serve_cmd="cargo run -- --foreground --verbose --persona ${BEACON_PERSONA:-orin}",
    deps=["src"],
    resource_deps=gateway_deps,
    labels=[project_name],
)

# Run tests
local_resource(
    "test-%s" % project_name,
    cmd="cargo test",
    deps=["src", "Cargo.toml"],
    labels=[project_name],
    auto_init=False,
    trigger_mode=TRIGGER_MODE_MANUAL,
)

# Lint
local_resource(
    "lint-%s" % project_name,
    cmd="cargo clippy",
    deps=["src", "Cargo.toml"],
    labels=[project_name],
    auto_init=False,
    trigger_mode=TRIGGER_MODE_MANUAL,
)
