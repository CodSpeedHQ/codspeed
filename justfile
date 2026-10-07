# Executor tests, in Docker containers
test-integ *args:
    tests/docker/run.sh {{args}}

# Remove the executor tests' Docker images, containers and cache volumes
clean-integ:
    docker rm -f codspeed-executor-tests-setup 2>/dev/null || true
    docker images --filter reference=codspeed-executor-tests --filter reference='codspeed-executor-tests:*' -q | sort -u | xargs -r docker rmi -f
    docker volume rm -f codspeed-tests-target codspeed-tests-cargo codspeed-tests-cargo-git
