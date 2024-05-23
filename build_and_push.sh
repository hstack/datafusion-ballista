#!/usr/bin/env bash
set -x
set -e

VERSION=${VERSION:-latest} #"MR202008"
PLATFORM=${PLATFORM:-"linux/amd64"} # build for ethos by default, replace with linux/arm64 for local macos builds
IMAGE_REPO_ADDRESS=${IMAGE_REPO_ADDRESS:-docker-cja-arrow-dev.dr-uw2.adobeitc.com}
DOCKER_BUILD_OPTIONS=${DOCKER_BUILD_OPTIONS:-""}

export DOCKER_BUILDKIT=1
export ARTIFACTORY_USER=${ARTIFACTORY_USER:-arrowops}
echo ${ARTIFACTORY_USER} > artifactory_user
export ARTIFACTORY_API_TOKEN=${ARTIFACTORY_API_TOKEN:-`vault kv get -field key dx_cja_arrow/arrowops_artifactory`}
echo ${ARTIFACTORY_API_TOKEN} > artifactory_api_token

cleanup() {
  rm -rf secret_*
  rm -rf artifactory_*
}

trap "cleanup" EXIT

IMG="ballista:${VERSION}"

docker build $DOCKER_BUILD_OPTIONS \
  --progress=plain \
  --platform=${PLATFORM} \
  --secret id=artifactory_user,src=./artifactory_user \
  --secret id=artifactory_api_token,src=./artifactory_api_token \
  -t ${IMG} .
if [[ $? -eq 0 ]]; then
  docker tag ${IMG} ${IMAGE_REPO_ADDRESS}/${IMG}
  # if we have NOPUSH, don't push and don't delete local images
  [[ -z "$NOPUSH" ]] && {
    docker push ${IMAGE_REPO_ADDRESS}/${IMG}
    docker images --format "{{.Repository}}:{{.Tag}} {{.ID}}" | grep ".\+/${IMG}" | awk '{print $1}' | xargs docker rmi -f
  }
  docker rmi -f ${IMG}
else
  echo "Docker build failed! See errors above."
  exit 1
fi
exit 0

