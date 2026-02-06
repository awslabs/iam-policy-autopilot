# Invalid Configuration Test Files

This directory contains intentionally malformed configuration files used for negative testing.

## Purpose

These files are used to verify that the configuration parsing logic correctly rejects invalid inputs during unit tests, ensuring that errors are caught during development rather than at runtime.

## How It Works

The negative tests use **RustEmbed** to embed the invalid test files, just like the production code embeds valid configuration files. The key difference is:

1. **Production code** uses RustEmbed pointing to `resources/config/` with valid files
2. **Test code** uses RustEmbed pointing to `tests/resources/invalid_configs/` with invalid files
