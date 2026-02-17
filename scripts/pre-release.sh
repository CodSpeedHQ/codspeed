#!/usr/bin/env bash
set -e

# First and only argument is the version number
VERSION=$1 # The version number, prefixed with 'v'

# Skip alpha/beta/rc changelog generation
if [[ $VERSION == *"alpha"* ]] || [[ $VERSION == *"beta"* ]] || [[ $VERSION == *"rc"* ]]; then
    echo "Skipping changelog generation for alpha/beta/rc release"
    exit 0
fi

# Check that GITHUB_TOKEN is set
if [ -z "$GITHUB_TOKEN" ]; then
    echo "GITHUB_TOKEN is not set. Trying to fetch it from gh"
    GITHUB_TOKEN=$(gh auth token)

fi

git cliff -o CHANGELOG.md --tag $VERSION --github-token $GITHUB_TOKEN
