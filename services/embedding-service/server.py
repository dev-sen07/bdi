from concurrent import futures
import grpc
import bdi_pb2
import bdi_pb2_grpc
from sentence_transformers import SentenceTransformer
import logging

logging.basicConfig(level=logging.INFO)

class EmbeddingService(bdi_pb2_grpc.EmbeddingServiceServicer):
    def __init__(self):
        logging.info("Loading model qwen3.5...")
        self.model = SentenceTransformer('all-MiniLM-L6-v2')
        logging.info("Model loaded.")

    def Encode(self, request, context):
        embedding = self.model.encode(request.text).tolist()
        return bdi_pb2.VectorResponse(vector=embedding)

def serve():
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
    bdi_pb2_grpc.add_EmbeddingServiceServicer_to_server(EmbeddingService(), server)
    server.add_insecure_port('[::]:50051')
    server.start()
    logging.info("EmbeddingService started on :50051")
    server.wait_for_termination()

if __name__ == '__main__':
    serve()
