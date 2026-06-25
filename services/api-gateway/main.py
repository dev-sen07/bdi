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

# Modelo por defecto si Ollama no estuviera accesible para listar lo instalado.
FALLBACK_MODEL = "qwen2.5:7b"

def _pretty_label(name: str) -> str:
    """Convierte un tag de Ollama (p.ej. 'qwen2.5:7b') en una etiqueta legible."""
    base, _, tag = name.partition(":")
    label = base.replace("-", " ").replace("_", " ").title()
    return f"{label} ({tag})" if tag else label

async def fetch_installed_models() -> list[dict]:
    """Lista los modelos REALMENTE instalados en Ollama (GET /api/tags), para que el
    selector del frontend solo ofrezca modelos disponibles localmente. Se consulta
    en vivo: si se hace `ollama pull` de otro modelo, aparece sin redeploy."""
    async with httpx.AsyncClient() as client:
        resp = await client.get(f"{LLM_SERVICE}/api/tags", timeout=10.0)
        resp.raise_for_status()
        data = resp.json()
    models = []
    for m in data.get("models", []):
        name = m.get("name") or m.get("model")
        if name:
            models.append({"id": name, "label": _pretty_label(name)})
    return models

class QueryRequest(BaseModel):
    question: str
    model: str | None = None

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
    if context_text.strip():
        prompt = (
            f"Eres un asistente conversacional. Tienes acceso a la siguiente información relevante:\n\n"
            f"{context_text}\n\n"
            f"Usa esa información para enriquecer tu respuesta de forma natural, sin mencionar que proviene de ninguna base de datos ni sistema. "
            f"Responde directamente a la pregunta como si lo supieras tú.\n\n"
            f"Pregunta: {request.question}\nRespuesta:"
        )
    else:
        prompt = (
            f"Eres un asistente conversacional amigable. No tienes información específica sobre el tema consultado. "
            f"Responde de forma amigable indicando que no tienes información al respecto y, si puedes, ofrece ayuda general.\n\n"
            f"Pregunta: {request.question}\nRespuesta:"
        )

    # 4. Llamar LLM Service (Ollama)
    # El modelo lo elige el usuario en la UI (el frontend solo ofrece los instalados).
    # Si no llega ninguno, caemos al primer modelo disponible en Ollama.
    model = request.model
    if not model:
        try:
            installed = await fetch_installed_models()
            model = installed[0]["id"] if installed else FALLBACK_MODEL
        except Exception:
            model = FALLBACK_MODEL
    sources = [r.id for r in results]
    try:
        async with httpx.AsyncClient() as client:
            llm_response = await client.post(
                f"{LLM_SERVICE}/api/generate",
                json={
                    "model": model,
                    "prompt": prompt,
                    "stream": False,
                    # Desactiva el "thinking": los modelos razonadores (p.ej. Qwen3)
                    # vuelcan su cadena de pensamiento al campo `thinking` y, con un
                    # tope de tokens, agotan el presupuesto pensando y dejan `response`
                    # vacío. Con think=false responden directo en `response`. Es inocuo
                    # para modelos que no razonan (lo ignoran).
                    "think": False,
                    "options": {
                        "num_predict": 512
                    }
                },
                timeout=300.0
            )
            llm_response.raise_for_status()
            answer = llm_response.json().get("response", "").strip()
            if not answer:
                answer = "El modelo no devolvió una respuesta. Prueba con otro modelo."
    except Exception as e:
        error_type = type(e).__name__
        answer = f"Error llamando al LLM ({error_type}: {str(e) or 'sin detalle'}). Contexto encontrado:\n{context_text}"

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

@app.get("/models")
async def list_models():
    """Modelos instalados en Ollama, para poblar dinámicamente el selector de la UI.
    Si Ollama no responde, devuelve lista vacía (el frontend lo refleja)."""
    try:
        models = await fetch_installed_models()
    except Exception:
        models = []
    default = models[0]["id"] if models else None
    return {"default": default, "models": models}

@app.get("/health")
def health():
    return {"status": "ok"}
