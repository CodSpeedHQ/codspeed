# Executor tests, in Docker containers
test-integ *args:
    tests/docker/run.sh {{args}}

# Remove the executor tests' Docker images, containers and cache volumes
clean-integ:
    docker ps -aq --filter name=codspeed-executor-tests-setup | xargs -r docker rm -f
    docker images --filter reference=codspeed-executor-tests --filter reference='codspeed-executor-tests:*' -q | sort -u | xargs -r docker rmi -f
    docker volume ls -q --filter name=codspeed-tests- | xargs -r docker volume rm -f
