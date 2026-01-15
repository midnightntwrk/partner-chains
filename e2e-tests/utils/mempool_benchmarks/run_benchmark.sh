#!/bin/bash
# Wrapper script to ensure virtual environment is properly set up before running benchmark

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

VENV_DIR="$SCRIPT_DIR/venv"
PYTHON_BIN="$VENV_DIR/bin/python3"
PIP_BIN="$VENV_DIR/bin/pip"

# Function to check if venv is valid
check_venv() {
    if [ -f "$PYTHON_BIN" ] && [ -f "$PIP_BIN" ]; then
        # Check if pandas is installed
        if "$PYTHON_BIN" -c "import pandas" 2>/dev/null; then
            return 0
        fi
    fi
    return 1
}

# Create or repair virtual environment if needed
if ! check_venv; then
    echo "Setting up virtual environment..."
    
    # Remove old venv if it exists but is broken
    if [ -d "$VENV_DIR" ]; then
        echo "Removing broken virtual environment..."
        rm -rf "$VENV_DIR"
    fi
    
    # Create fresh virtual environment
    echo "Creating new virtual environment..."
    python3 -m venv "$VENV_DIR"
    
    # Install dependencies
    echo "Installing dependencies..."
    "$PIP_BIN" install --upgrade pip
    "$PIP_BIN" install -r requirements.txt
    
    echo "Virtual environment setup complete!"
    echo ""
fi

# Run the benchmark script with all arguments passed through
exec "$PYTHON_BIN" "$SCRIPT_DIR/run_mempool_benchmark.py" "$@"
