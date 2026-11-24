# shellcheck shell=bash
# box up: image, network and proxy.

readonly BUILD_TIMEOUT_SECONDS=1800
readonly COMPOSE_TIMEOUT_SECONDS=300

cmd_up() {
  if [ "${1:-}" = "--check" ]; then up_check_json; return; fi
  require_docker
  if ! dk image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "building $IMAGE"
    run_with_timeout "$BUILD_TIMEOUT_SECONDS" docker build -t "$IMAGE" "$BOX_ROOT/image"
  fi
  run_with_timeout "$COMPOSE_TIMEOUT_SECONDS" docker compose -f "$CONTAINER_RESOURCES/compose.yaml" up -d --build
  echo "agentbox is up: network $NETWORK, proxy $PROXY_CONTAINER"
}

# box up --check: whether a box can be used now, as one JSON object, from three quick questions to
# Docker; nothing is built or started. {docker, image, proxy, ready}: ready is all three.
up_check_json() {
  local docker=false image=false proxy=false
  if dk info >/dev/null 2>&1; then
    docker=true
    ! dk image inspect "$IMAGE" >/dev/null 2>&1 || image=true
    [ "$(dk inspect -f '{{.State.Running}}' "$PROXY_CONTAINER" 2>/dev/null)" != "true" ] || proxy=true
  fi
  jq -nc --argjson docker "$docker" --argjson image "$image" --argjson proxy "$proxy" \
    '{docker: $docker, image: $image, proxy: $proxy, ready: ($docker and $image and $proxy)}'
}
