from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel
import httpx
import os
import grpc_clients

app = FastAPI(title="BDI API Gateway")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"], # For local development
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

LLM_SERVICE = os.getenv("LLM_SERVICE", "http://localhost:11434")

class QueryRequest(BaseModel):
    question: str

class IngestRequest(BaseModel):
    text: str

class UpdateNodeRequest(BaseModel):
    text: str

@app.post("/query")
async def query(request: QueryRequest):
    # 1. Vectorize
    try:
        embedding = grpc_clients.get_embedding(request.question)
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Embedding error: {str(e)}")

    # 2. Query Rust Engine
    try:
        results = grpc_clients.engine_query(request.question, embedding)
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Engine query error: {str(e)}")

    # 3. Construir Prompt con los nodos recuperados
    context_text = "\n".join([f"- {r.content}" for r in results])
    prompt = f"Usa el siguiente contexto de nuestra base de datos inteligente para responder a la pregunta de forma precisa. Si el contexto no tiene la respuesta, usa tu conocimiento pero menciona que no está en la base de datos.\n\nContexto:\n{context_text}\n\nPregunta: {request.question}\nRespuesta:"

    # 4. Llamar LLM Service (Ollama)
    sources = [r.id for r in results]
    try:
        async with httpx.AsyncClient() as client:
            llm_response = await client.post(
                f"{LLM_SERVICE}/api/generate",
                json={
                    "model": "llama3.1:8b",
                    "prompt": prompt,
                    "stream": False
                },
                timeout=60.0
            )
            llm_response.raise_for_status()
            answer = llm_response.json().get("response", "")
    except Exception as e:
        answer = f"Error llamando al LLM: {str(e)}. Contexto encontrado: {context_text}"

    return {
        "answer": answer,
        "sources": sources
    }

@app.post("/ingest")
async def ingest(request: IngestRequest):
    try:
        embedding = grpc_clients.get_embedding(request.text)
        res = grpc_clients.engine_ingest(request.text, embedding)
        return {
            "action": res.action,
            "node_id": res.node_id
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))

@app.get("/stats")
async def stats():
    try:
        res = grpc_clients.engine_stats()
        return {
            "total_nodes": res.total_nodes,
            "total_edges": res.total_edges,
            "forgotten_nodes": res.forgotten_nodes
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))

@app.get("/nodes")
async def list_nodes(limit: int = Query(50, ge=1, le=500), offset: int = Query(0, ge=0)):
    """Lista paginada del conocimiento almacenado, para administrarlo en la UI."""
    try:
        res = grpc_clients.engine_list_nodes(limit=limit, offset=offset)
        return {
            "total": res.total,
            "nodes": [
                {
                    "id": n.id,
                    "content": n.content,
                    "uso_count": n.uso_count,
                    "merge_count": n.merge_count,
                    "created_at": n.created_at,
                    "last_accessed": n.last_accessed,
                    "degree": n.degree,
                }
                for n in res.nodes
            ],
        }
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))

@app.put("/nodes/{node_id}")
async def update_node(node_id: str, request: UpdateNodeRequest):
    """Edita el contenido de un nodo. Re-vectoriza el nuevo texto para mantener
    el índice vectorial consistente con el contenido editado."""
    text = request.text.strip()
    if not text:
        raise HTTPException(status_code=400, detail="El texto no puede estar vacío.")
    try:
        embedding = grpc_clients.get_embedding(text)
        res = grpc_clients.engine_update_node(node_id, text, embedding)
        if not res.success:
            raise HTTPException(status_code=404, detail=f"Nodo {node_id} no encontrado.")
        return {"success": True, "id": res.id}
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))

@app.delete("/nodes/{node_id}")
async def delete_node(node_id: str):
    """Elimina un nodo y sus aristas del grafo (olvido manual desde la UI)."""
    try:
        res = grpc_clients.engine_delete_node(node_id)
        if not res.success:
            raise HTTPException(status_code=404, detail=f"Nodo {node_id} no encontrado.")
        return {"success": True, "deleted_edges": res.deleted_edges}
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))

@app.get("/health")
def health():
    return {"status": "ok"}
