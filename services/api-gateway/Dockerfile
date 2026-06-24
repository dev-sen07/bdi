FROM python:3.11-slim

WORKDIR /app

RUN apt-get update && apt-get install -y gcc g++ && rm -rf /var/lib/apt/lists/*

COPY services/api-gateway/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copiar proto y generar
COPY proto /proto
RUN python -m grpc_tools.protoc -I/proto --python_out=. --grpc_python_out=. /proto/bdi.proto

COPY services/api-gateway .

EXPOSE 8000

CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8000"]
