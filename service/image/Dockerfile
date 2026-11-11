FROM node:24-bookworm-slim

ARG CLAUDE_CODE_VERSION=2.1.321
ARG CODEX_VERSION=0.167.0
ARG PLAYWRIGHT_VERSION=1.65.0
# The Claude ACP adapter behind `box acp`. Its Claude Agent SDK is released with the CLI and shares its
# patch number (SDK 0.3.321 goes with Claude Code 2.1.321): bump the two together.
ARG CLAUDE_AGENT_ACP_VERSION=0.103.0

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates curl procps python3 \
 && rm -rf /var/lib/apt/lists/*

# Headless Chromium only (no full browser), with the system libraries it needs.
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/playwright
RUN npx -y "playwright@${PLAYWRIGHT_VERSION}" install --with-deps chromium-headless-shell \
 && ln -s "$(find /opt/playwright -name chrome-headless-shell -type f | head -1)" /usr/local/bin/chrome-headless-shell \
 && rm -rf /var/lib/apt/lists/* /root/.npm
ENV CHROME_HEADLESS_SHELL=/usr/local/bin/chrome-headless-shell

RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" "@openai/codex@${CODEX_VERSION}" \
 && npm cache clean --force

# The adapter drives the Claude Code CLI installed above (box acp sets CLAUDE_CODE_EXECUTABLE), so
# the second copy of the CLI that its SDK brings as a platform package (326 MB) is removed.
RUN npm install -g "@agentclientprotocol/claude-agent-acp@${CLAUDE_AGENT_ACP_VERSION}" \
 && rm -rf /usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai/claude-agent-sdk-linux-* \
 && npm cache clean --force

# Non-root user. The directories below are owned by it so that fresh named volumes
# mounted on them inherit the ownership.
RUN usermod -l agent -d /home/agent -m node && groupmod -n agent node \
 && mkdir -p /work /ws /deps /npm-cache /acct \
 && chown agent:agent /work /ws /deps /npm-cache /acct \
 && git config --system safe.directory '*'

COPY verify /usr/local/bin/verify

ENV HOME=/home/agent \
    npm_config_cache=/npm-cache \
    NODE_USE_ENV_PROXY=1 \
    CHROME_BIN=/usr/local/bin/chrome-headless-shell
USER agent
WORKDIR /work
