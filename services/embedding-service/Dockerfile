FROM python:3.11-slim

WORKDIR /app

# Instalar dependencias del sistema para compilar librerías de Python
RUN apt-get update && apt-get install -y build-essential gcc g++ && rm -rf /var/lib/apt/lists/*

COPY services/embedding-service/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copiar el .proto para generar los archivos de Python
COPY proto /proto

# Generar clases gRPC de Python
RUN python -m grpc_tools.protoc -I/proto --python_out=. --grpc_python_out=. /proto/bdi.proto

COPY services/embedding-service/server.py .

EXPOSE 50051

CMD ["python", "server.py"]
