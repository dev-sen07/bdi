#!/bin/bash
set -e

# Modelos que expone el chat. Deben coincidir con AVAILABLE_MODELS del api-gateway.
MODELS=("qwen3.5:9b" "llama3.1:8b")

ollama serve &
SERVER_PID=$!

# Espera a que el servidor responda antes de descargar nada.
until ollama list > /dev/null 2>&1; do sleep 2; done

# Descarga cada modelo solo si no está ya en el volumen.
for model in "${MODELS[@]}"; do
    if ! ollama list | grep -q "${model}"; then
        echo "Descargando ${model}..."
        ollama pull "${model}"
    fi
done

# Pre-carga los modelos en GPU para que la primera consulta no pague el cold-start.
# Con OLLAMA_KEEP_ALIVE=-1 quedan residentes (qwen ~4.7GB + mistral ~4.1GB caben en 16GB).
for model in "${MODELS[@]}"; do
    ollama run "${model}" "" > /dev/null 2>&1 || true
done

wait $SERVER_PID
