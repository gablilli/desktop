#!/bin/bash

# Convenience script to run the Cloudreve Sync Service

echo "🚀 Starting Cloudreve Sync Service..."
echo ""
echo "The service will start on http://127.0.0.1:3000"
echo "Open examples/client.html in your browser to use the GUI client"
echo ""
echo "Press Ctrl+C to stop the service"
echo ""

# Run with info level logging by default
RUST_LOG=info cargo run --release

