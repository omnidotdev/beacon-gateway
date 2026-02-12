#!/usr/bin/env bash
# Publish a persona to Manifold registry
#
# Usage: ./scripts/publish-persona.sh <persona-id> [--token <token>]
#
# Environment variables:
#   MANIFOLD_URL       - Manifold API URL (default: https://api.manifold.omni.dev)
#   MANIFOLD_NAMESPACE - Namespace to publish to (default: omni)
#   MANIFOLD_TOKEN     - Auth token (required)

set -euo pipefail

MANIFOLD_URL="${MANIFOLD_URL:-https://api.manifold.omni.dev}"
MANIFOLD_NAMESPACE="${MANIFOLD_NAMESPACE:-omni}"
MANIFOLD_TOKEN="${MANIFOLD_TOKEN:-}"

usage() {
  echo "Usage: $0 <persona-id> [--token <token>]"
  echo ""
  echo "Publishes a persona JSON file to Manifold registry."
  echo ""
  echo "Arguments:"
  echo "  persona-id    The persona ID (e.g., 'orin')"
  echo ""
  echo "Options:"
  echo "  --token       Auth token (or set MANIFOLD_TOKEN env var)"
  echo ""
  echo "Environment:"
  echo "  MANIFOLD_URL       API URL (default: https://api.manifold.omni.dev)"
  echo "  MANIFOLD_NAMESPACE Namespace (default: omni)"
  exit 1
}

# Parse arguments
PERSONA_ID=""
while [[ $# -gt 0 ]]; do
  case $1 in
    --token)
      MANIFOLD_TOKEN="$2"
      shift 2
      ;;
    -h|--help)
      usage
      ;;
    *)
      PERSONA_ID="$1"
      shift
      ;;
  esac
done

if [[ -z "$PERSONA_ID" ]]; then
  echo "Error: persona-id required"
  usage
fi

if [[ -z "$MANIFOLD_TOKEN" ]]; then
  echo "Error: MANIFOLD_TOKEN required (set env var or use --token)"
  exit 1
fi

# Find persona file
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
PERSONA_FILE="$REPO_ROOT/personas/${PERSONA_ID}.json"

if [[ ! -f "$PERSONA_FILE" ]]; then
  echo "Error: Persona file not found: $PERSONA_FILE"
  exit 1
fi

# Read and prepare content
CONTENT=$(cat "$PERSONA_FILE")
# Escape for JSON (handle quotes and newlines)
CONTENT_ESCAPED=$(echo "$CONTENT" | jq -Rs .)
DIGEST="sha256:$(echo -n "$CONTENT" | sha256sum | cut -d' ' -f1)"
SIZE=$(echo -n "$CONTENT" | wc -c)

echo "Publishing persona: $PERSONA_ID"
echo "  File: $PERSONA_FILE"
echo "  Namespace: $MANIFOLD_NAMESPACE"
echo "  Repository: personas"
echo "  Digest: $DIGEST"
echo "  Size: $SIZE bytes"
echo ""

# GraphQL helper
gql() {
  local query="$1"
  local variables="${2:-{}}"

  curl -s "$MANIFOLD_URL/graphql" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $MANIFOLD_TOKEN" \
    -d "{\"query\": $(echo "$query" | jq -Rs .), \"variables\": $variables}"
}

# Step 1: Get or create namespace
echo "Step 1: Ensuring namespace exists..."
NS_RESULT=$(gql '
  query GetNamespace($name: String!) {
    namespaces(condition: { name: $name }) {
      nodes { rowId name }
    }
  }
' "{\"name\": \"$MANIFOLD_NAMESPACE\"}")

NS_ID=$(echo "$NS_RESULT" | jq -r '.data.namespaces.nodes[0].rowId // empty')

if [[ -z "$NS_ID" ]]; then
  echo "  Creating namespace: $MANIFOLD_NAMESPACE"
  NS_CREATE=$(gql '
    mutation CreateNamespace($name: String!) {
      createNamespace(input: { namespace: { name: $name } }) {
        namespace { rowId }
      }
    }
  ' "{\"name\": \"$MANIFOLD_NAMESPACE\"}")

  NS_ID=$(echo "$NS_CREATE" | jq -r '.data.createNamespace.namespace.rowId // empty')
  if [[ -z "$NS_ID" ]]; then
    echo "  Error creating namespace:"
    echo "$NS_CREATE" | jq .
    exit 1
  fi
fi
echo "  Namespace ID: $NS_ID"

# Step 2: Get or create repository
echo "Step 2: Ensuring repository exists..."
REPO_RESULT=$(gql '
  query GetRepository($nsId: UUID!, $name: String!) {
    repositories(condition: { namespaceId: $nsId, name: $name }) {
      nodes { rowId name }
    }
  }
' "{\"nsId\": \"$NS_ID\", \"name\": \"personas\"}")

REPO_ID=$(echo "$REPO_RESULT" | jq -r '.data.repositories.nodes[0].rowId // empty')

if [[ -z "$REPO_ID" ]]; then
  echo "  Creating repository: personas"
  REPO_CREATE=$(gql '
    mutation CreateRepository($nsId: UUID!, $name: String!) {
      createRepository(input: { repository: { namespaceId: $nsId, name: $name, artifactType: "persona" } }) {
        repository { rowId }
      }
    }
  ' "{\"nsId\": \"$NS_ID\", \"name\": \"personas\"}")

  REPO_ID=$(echo "$REPO_CREATE" | jq -r '.data.createRepository.repository.rowId // empty')
  if [[ -z "$REPO_ID" ]]; then
    echo "  Error creating repository:"
    echo "$REPO_CREATE" | jq .
    exit 1
  fi
fi
echo "  Repository ID: $REPO_ID"

# Step 3: Create artifact
echo "Step 3: Creating artifact..."
ARTIFACT_CREATE=$(gql "
  mutation CreateArtifact(\$repoId: UUID!, \$digest: String!, \$size: BigInt!, \$content: String!) {
    createArtifact(input: { artifact: { repositoryId: \$repoId, digest: \$digest, size: \$size, mediaType: \"application/json\", content: \$content } }) {
      artifact { rowId digest }
    }
  }
" "{\"repoId\": \"$REPO_ID\", \"digest\": \"$DIGEST\", \"size\": $SIZE, \"content\": $CONTENT_ESCAPED}")

ARTIFACT_ID=$(echo "$ARTIFACT_CREATE" | jq -r '.data.createArtifact.artifact.rowId // empty')
if [[ -z "$ARTIFACT_ID" ]]; then
  # Check if artifact already exists (same digest)
  echo "  Artifact may already exist, checking..."
  ARTIFACT_RESULT=$(gql '
    query GetArtifact($repoId: UUID!, $digest: String!) {
      artifacts(condition: { repositoryId: $repoId, digest: $digest }) {
        nodes { rowId }
      }
    }
  ' "{\"repoId\": \"$REPO_ID\", \"digest\": \"$DIGEST\"}")

  ARTIFACT_ID=$(echo "$ARTIFACT_RESULT" | jq -r '.data.artifacts.nodes[0].rowId // empty')
  if [[ -z "$ARTIFACT_ID" ]]; then
    echo "  Error creating artifact:"
    echo "$ARTIFACT_CREATE" | jq .
    exit 1
  fi
  echo "  Artifact already exists with same content"
fi
echo "  Artifact ID: $ARTIFACT_ID"

# Step 4: Create or update tag
echo "Step 4: Creating/updating tag..."
# Check if tag exists
TAG_RESULT=$(gql '
  query GetTag($repoId: UUID!, $name: String!) {
    tags(condition: { repositoryId: $repoId, name: $name }) {
      nodes { rowId artifactId }
    }
  }
' "{\"repoId\": \"$REPO_ID\", \"name\": \"$PERSONA_ID\"}")

TAG_ID=$(echo "$TAG_RESULT" | jq -r '.data.tags.nodes[0].rowId // empty')

if [[ -n "$TAG_ID" ]]; then
  # Update existing tag
  echo "  Updating existing tag: $PERSONA_ID"
  TAG_UPDATE=$(gql '
    mutation UpdateTag($tagId: UUID!, $artifactId: UUID!) {
      updateTag(input: { rowId: $tagId, patch: { artifactId: $artifactId } }) {
        tag { rowId name }
      }
    }
  ' "{\"tagId\": \"$TAG_ID\", \"artifactId\": \"$ARTIFACT_ID\"}")

  if ! echo "$TAG_UPDATE" | jq -e '.data.updateTag.tag.rowId' > /dev/null; then
    echo "  Error updating tag:"
    echo "$TAG_UPDATE" | jq .
    exit 1
  fi
else
  # Create new tag
  echo "  Creating tag: $PERSONA_ID"
  TAG_CREATE=$(gql '
    mutation CreateTag($repoId: UUID!, $artifactId: UUID!, $name: String!) {
      createTag(input: { tag: { repositoryId: $repoId, artifactId: $artifactId, name: $name } }) {
        tag { rowId name }
      }
    }
  ' "{\"repoId\": \"$REPO_ID\", \"artifactId\": \"$ARTIFACT_ID\", \"name\": \"$PERSONA_ID\"}")

  if ! echo "$TAG_CREATE" | jq -e '.data.createTag.tag.rowId' > /dev/null; then
    echo "  Error creating tag:"
    echo "$TAG_CREATE" | jq .
    exit 1
  fi
fi

echo ""
echo "Successfully published $PERSONA_ID to $MANIFOLD_NAMESPACE/personas:$PERSONA_ID"
