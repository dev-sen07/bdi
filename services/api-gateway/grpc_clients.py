import grpc
import bdi_pb2
import bdi_pb2_grpc
import os

EMBEDDING_SERVICE = os.getenv("EMBEDDING_SERVICE", "localhost:50051")
RUST_ENGINE = os.getenv("RUST_ENGINE", "localhost:50052")

def get_embedding(text: str) -> list[float]:
    channel = grpc.insecure_channel(EMBEDDING_SERVICE)
    stub = bdi_pb2_grpc.EmbeddingServiceStub(channel)
    request = bdi_pb2.TextRequest(text=text)
    response = stub.Encode(request)
    return list(response.vector)

def engine_query(text: str, embedding: list[float], top_k: int = 5):
    channel = grpc.insecure_channel(RUST_ENGINE)
    stub = bdi_pb2_grpc.GraphEngineStub(channel)
    request = bdi_pb2.QueryRequest(text=text, top_k=top_k, embedding=embedding)
    response = stub.Query(request)
    return response.results

def engine_ingest(text: str, embedding: list[float]):
    channel = grpc.insecure_channel(RUST_ENGINE)
    stub = bdi_pb2_grpc.GraphEngineStub(channel)
    request = bdi_pb2.IngestRequest(text=text, embedding=embedding)
    response = stub.Ingest(request)
    return response

def engine_stats():
    channel = grpc.insecure_channel(RUST_ENGINE)
    stub = bdi_pb2_grpc.GraphEngineStub(channel)
    request = bdi_pb2.StatsRequest()
    response = stub.GetStats(request)
    return response

def engine_list_nodes(limit: int = 50, offset: int = 0):
    channel = grpc.insecure_channel(RUST_ENGINE)
    stub = bdi_pb2_grpc.GraphEngineStub(channel)
    request = bdi_pb2.ListRequest(limit=limit, offset=offset)
    response = stub.ListNodes(request)
    return response

def engine_update_node(node_id: str, text: str, embedding: list[float]):
    channel = grpc.insecure_channel(RUST_ENGINE)
    stub = bdi_pb2_grpc.GraphEngineStub(channel)
    request = bdi_pb2.UpdateRequest(id=node_id, text=text, embedding=embedding)
    response = stub.UpdateNode(request)
    return response

def engine_delete_node(node_id: str):
    channel = grpc.insecure_channel(RUST_ENGINE)
    stub = bdi_pb2_grpc.GraphEngineStub(channel)
    request = bdi_pb2.DeleteRequest(id=node_id)
    response = stub.DeleteNode(request)
    return response
