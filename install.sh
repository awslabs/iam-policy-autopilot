#!/bin/sh
# IAM Policy Autopilot Installation Script
#
# This script automatically downloads and installs the IAM Policy Autopilot CLI tool
# for Unix-like systems (macOS and Linux).
#
# Supported platforms:
#   - macOS (Darwin): x86_64, ARM64
#   - Linux: x86_64, ARM64
#
# Usage:
#   # Fresh installation (one-liner):
#   curl -sSL https://github.com/awslabs/iam-policy-autopilot/raw/refs/heads/main/install.sh | sh
#
#   # Update existing installation (same command):
#   curl -sSL https://github.com/awslabs/iam-policy-autopilot/raw/refs/heads/main/install.sh | sh
#
#   # Manual installation with sudo:
#   curl -sSL https://github.com/awslabs/iam-policy-autopilot/raw/refs/heads/main/install.sh | sudo sh
#
#   # Download and run locally:
#   curl -sSL https://github.com/awslabs/iam-policy-autopilot/raw/refs/heads/main/install.sh -o install.sh
#   chmod +x install.sh
#   ./install.sh
#
# The script will automatically detect if you have an existing installation and update it
# if a newer version is available. If you're already running the latest version, it will
# notify you and exit without making changes.

# Repository configuration
GITHUB_ORG="awslabs"
GITHUB_REPO="iam-policy-autopilot"

set -e

# Global variables for cleanup
TEMP_FILE=""
TEMP_DIR=""

# Cleanup function
cleanup() {
    if [ -n "$TEMP_FILE" ] && [ -f "$TEMP_FILE" ]; then
        rm -f "$TEMP_FILE"
    fi
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf "$TEMP_DIR"
    fi
}

# Set up cleanup trap for temporary files
trap cleanup EXIT INT TERM

# Error handling function
error() {
    echo "Error: $1" >&2
    cleanup
    exit 1
}

# Platform detection function
detect_platform() {
    echo "Detecting platform..."

    # Detect OS
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')

    case "$OS" in
        darwin)
            OS="apple-darwin"
            ;;
        linux)
            OS="unknown-linux-gnu"
            ;;
        *)
            error "Unsupported operating system: $OS. Supported: macOS (Darwin), Linux"
            ;;
    esac

    # Detect architecture
    ARCH=$(uname -m)

    case "$ARCH" in
        x86_64)
            # Keep as is
            ;;
        aarch64|arm64)
            ARCH="aarch64"
            ;;
        i686|i386)
            error "32-bit x86 architecture is not supported. Supported: x86_64, ARM64"
            ;;
        *)
            error "Unsupported architecture: $ARCH. Supported: x86_64, ARM64"
            ;;
    esac

    # Construct platform target triple
    PLATFORM="${ARCH}-${OS}"

    echo "Detected platform: $PLATFORM"
}

# GitHub API integration function
get_latest_release() {
    echo "Fetching latest release information from GitHub..."

    # Query GitHub API for latest release
    RELEASE_JSON=$(curl -sL \
        -H "Accept: application/vnd.github+json" \
        -H "X-GitHub-Api-Version: 2022-11-28" \
        "https://api.github.com/repos/$GITHUB_ORG/$GITHUB_REPO/releases/latest" 2>&1)

    # Check if curl command succeeded
    if [ $? -ne 0 ]; then
        error "Failed to fetch release information from GitHub. Please check your internet connection and try again."
    fi

    # Check if response contains error
    if echo "$RELEASE_JSON" | grep -q '"message".*"Not Found"'; then
        error "GitHub repository or release not found. Please verify the repository exists."
    fi

    if echo "$RELEASE_JSON" | grep -q '"message".*"API rate limit exceeded"'; then
        error "GitHub API rate limit exceeded. Please try again later."
    fi

    # Extract version tag
    VERSION=$(echo "$RELEASE_JSON" | grep '"tag_name"' | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' | head -n 1)

    if [ -z "$VERSION" ]; then
        error "Failed to parse version from GitHub API response. The response may be malformed."
    fi

    echo "Latest version: $VERSION"

    # Find matching binary asset based on platform
    # Look for assets that contain the platform string in their name
    ASSET_URL=$(echo "$RELEASE_JSON" | grep '"browser_download_url"' | grep "$PLATFORM" | sed -E 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' | head -n 1)

    if [ -z "$ASSET_URL" ]; then
        error "No binary found for platform: $PLATFORM. This platform may not be supported yet. Please report this at https://github.com/$GITHUB_ORG/$GITHUB_REPO/issues"
    fi

    # Extract asset filename from URL
    ASSET_NAME=$(basename "$ASSET_URL")

    echo "Found binary: $ASSET_NAME"
    echo "Download URL: $ASSET_URL"
}

# Version comparison function
check_existing_version() {
    INSTALL_DIR="/usr/local/bin"
    BINARY_PATH="$INSTALL_DIR/iam-policy-autopilot"

    echo "Checking for existing installation..."

    # Check if binary exists in installation directory
    if [ -f "$BINARY_PATH" ]; then
        echo "Found existing installation at $BINARY_PATH"

        # Try to get installed version by executing --version command
        INSTALLED_VERSION=$("$BINARY_PATH" --version 2>/dev/null | awk '{print $2}')

        # If version detection failed, proceed with installation
        if [ -z "$INSTALLED_VERSION" ]; then
            echo "Could not determine installed version. Proceeding with installation."
            return 0
        fi

        echo "Installed version: $INSTALLED_VERSION"

        # Remove 'v' prefix from VERSION if present for comparison
        LATEST_VERSION_CLEAN="${VERSION#v}"

        # Compare installed version with latest release version
        if [ "$INSTALLED_VERSION" = "$LATEST_VERSION_CLEAN" ]; then
            echo ""
            echo "iam-policy-autopilot $INSTALLED_VERSION is already installed and up-to-date"
            exit 0
        fi

        echo "Updating iam-policy-autopilot from $INSTALLED_VERSION to $LATEST_VERSION_CLEAN"
    else
        echo "No existing installation found"
        echo "Installing iam-policy-autopilot ${VERSION#v}"
    fi
}

# Download binary function
download_binary() {
    echo ""
    echo "Downloading binary..."

    # Create temporary file for download
    TEMP_FILE=$(mktemp)

    echo "Downloading from $ASSET_URL"

    # Try curl first, fallback to wget
    if command -v curl >/dev/null 2>&1; then
        # Use curl with progress bar
        if ! curl -L --progress-bar -o "$TEMP_FILE" "$ASSET_URL"; then
            error "Failed to download binary using curl. Please check your internet connection and try again."
        fi
    elif command -v wget >/dev/null 2>&1; then
        # Use wget with progress display
        if ! wget -q --show-progress -O "$TEMP_FILE" "$ASSET_URL"; then
            error "Failed to download binary using wget. Please check your internet connection and try again."
        fi
    else
        error "Neither curl nor wget found. Please install curl or wget and try again."
    fi

    # Verify download succeeded by checking file size
    if [ ! -s "$TEMP_FILE" ]; then
        error "Downloaded file appears to be empty or corrupted. Please try again."
    fi

    echo "Download complete"

    # Handle archived formats
    # Check if the downloaded file is a tar.gz archive
    if echo "$ASSET_NAME" | grep -q "\.tar\.gz$"; then
        echo "Extracting tar.gz archive..."

        # Create temporary directory for extraction
        TEMP_DIR=$(mktemp -d)

        # Extract archive
        if ! tar -xzf "$TEMP_FILE" -C "$TEMP_DIR"; then
            error "Failed to extract tar.gz archive. The file may be corrupted."
        fi

        # Find the binary in the extracted files
        # Look for file named iam-policy-autopilot (without extension)
        EXTRACTED_BINARY=$(find "$TEMP_DIR" -type f -name "iam-policy-autopilot" | head -n 1)

        if [ -z "$EXTRACTED_BINARY" ]; then
            error "Could not find iam-policy-autopilot binary in extracted archive."
        fi

        # Move extracted binary to temp file location
        mv "$EXTRACTED_BINARY" "$TEMP_FILE"

        # Clean up extraction directory
        rm -rf "$TEMP_DIR"

        echo "Extraction complete"
    elif echo "$ASSET_NAME" | grep -q "\.zip$"; then
        echo "Extracting zip archive..."

        # Create temporary directory for extraction
        TEMP_DIR=$(mktemp -d)

        # Check if unzip is available
        if ! command -v unzip >/dev/null 2>&1; then
            error "unzip command not found. Please install unzip to extract .zip archives."
        fi

        # Extract archive
        if ! unzip -q "$TEMP_FILE" -d "$TEMP_DIR"; then
            error "Failed to extract zip archive. The file may be corrupted."
        fi

        # Find the binary in the extracted files
        EXTRACTED_BINARY=$(find "$TEMP_DIR" -type f -name "iam-policy-autopilot" | head -n 1)

        if [ -z "$EXTRACTED_BINARY" ]; then
            error "Could not find iam-policy-autopilot binary in extracted archive."
        fi

        # Move extracted binary to temp file location
        mv "$EXTRACTED_BINARY" "$TEMP_FILE"

        # Clean up extraction directory
        rm -rf "$TEMP_DIR"

        echo "Extraction complete"
    fi

    # Verify the final binary file exists and is not empty
    if [ ! -s "$TEMP_FILE" ]; then
        error "Binary file is missing or empty after processing. Please try again."
    fi

    echo "Binary ready for installation"
}

# Installation function
install_binary() {
    INSTALL_DIR="/usr/local/bin"
    BINARY_NAME="iam-policy-autopilot"
    INSTALL_PATH="$INSTALL_DIR/$BINARY_NAME"

    echo ""
    echo "Installing binary..."

    # Check write permissions for installation directory
    if [ ! -w "$INSTALL_DIR" ]; then
        error "No write permission to $INSTALL_DIR. Try running with sudo: curl -sSL https://github.com/$GITHUB_ORG/$GITHUB_REPO/raw/refs/heads/main/install.sh | sudo sh"
    fi

    # Preserve permissions when updating existing installation
    # If binary already exists, save its permissions
    EXISTING_PERMS=""
    if [ -f "$INSTALL_PATH" ]; then
        # Get current permissions in octal format (e.g., 755)
        EXISTING_PERMS=$(stat -f "%Lp" "$INSTALL_PATH" 2>/dev/null || stat -c "%a" "$INSTALL_PATH" 2>/dev/null)
        echo "Preserving existing permissions: $EXISTING_PERMS"
    fi

    # Move downloaded binary to installation directory
    if ! mv "$TEMP_FILE" "$INSTALL_PATH"; then
        error "Failed to move binary to $INSTALL_PATH. Please check permissions and try again."
    fi

    # Set executable permissions
    # If we preserved permissions from existing installation, restore them
    # Otherwise, set default executable permissions (755)
    if [ -n "$EXISTING_PERMS" ]; then
        chmod "$EXISTING_PERMS" "$INSTALL_PATH"
    else
        chmod +x "$INSTALL_PATH"
    fi

    # Get the installed version for success message
    FINAL_VERSION=$("$INSTALL_PATH" --version 2>/dev/null | awk '{print $2}' || echo "${VERSION#v}")

    echo ""
    echo "=========================================="
    echo "Installation complete!"
    echo "=========================================="
    echo "Version: $FINAL_VERSION"
    echo "Location: $INSTALL_PATH"
    echo ""
    echo "Run 'iam-policy-autopilot --help' to get started"
}

# Main execution
main() {
    detect_platform
    get_latest_release
    check_existing_version
    download_binary
    install_binary
}

main
