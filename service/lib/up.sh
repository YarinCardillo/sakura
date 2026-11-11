# shellcheck shell=bash
# box up: image, network and proxy.

readonly BUILD_TIMEOUT_SECONDS=1800
readonly COMPOSE_TIMEOUT_SECONDS=300

cmd_up() {
  require_docker
  if ! dk image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "building $IMAGE"
    run_with_timeout "$BUILD_TIMEOUT_SECONDS" docker build -t "$IMAGE" "$BOX_ROOT/image"
  fi
  run_with_timeout "$COMPOSE_TIMEOUT_SECONDS" docker compose -f "$CONTAINER_RESOURCES/compose.yaml" up -d --build
  echo "agentbox is up: network $NETWORK, proxy $PROXY_CONTAINER"
}
