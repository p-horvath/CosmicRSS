FROM docker.io/library/fedora:45

# System dependencies for building cosmic-rss
RUN dnf install -y --setopt=install_weak_deps=False\
        git-core \
        rust \
        cargo \
        libxkbcommon-devel \
    && dnf clean all
