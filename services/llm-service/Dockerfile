FROM ollama/ollama:latest

# Instalar y precargar Llama 3.1 8B. 
# Para esto necesitamos levantar el server de ollama en background durante el build.
RUN nohup bash -c "ollama serve &" && sleep 5 && ollama pull llama3.1:8b

# El entrypoint y cmd por defecto de ollama levantan el servicio HTTP en el puerto 11434.
