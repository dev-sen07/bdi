import os
import requests
import time

GATEWAY_URL = os.getenv("GATEWAY_URL", "http://localhost:8000")

def wait_for_gateway():
    print(f"Waiting for API Gateway at {GATEWAY_URL}...")
    for _ in range(30):
        try:
            r = requests.get(f"{GATEWAY_URL}/stats")
            if r.status_code == 200:
                print("API Gateway and Rust Engine are up!")
                return True
        except Exception:
            pass
        time.sleep(2)
    print("API Gateway is not responding.")
    return False

def test_ingest_and_query():
    # 1. Ingestar conocimiento (Dataset Mock de LoCoMo simplificado)
    facts = [
        "El autor de la tesis BDI es Miguel Quispe.",
        "El motor de base de datos BDI está escrito en Rust.",
        "El modelo LLM usado es Llama 3.1 8B y corre en Ollama.",
        "El sistema BDI usa embeddings all-MiniLM-L6-v2."
    ]

    for fact in facts:
        r = requests.post(f"{GATEWAY_URL}/ingest", json={"text": fact})
        assert r.status_code == 200
        print(f"Ingested: {fact} -> {r.json()}")

    time.sleep(1)

    # 2. Consultar
    questions = [
        ("¿Quién es el autor de la tesis BDI?", "Miguel Quispe"),
        ("¿En qué lenguaje está escrito el motor BDI?", "Rust"),
    ]

    correct = 0
    for q, expected in questions:
        r = requests.post(f"{GATEWAY_URL}/query", json={"question": q})
        assert r.status_code == 200
        ans = r.json().get("answer", "")
        print(f"Q: {q}\nA: {ans}\n")
        
        # Evaluación sencilla
        if expected.lower() in ans.lower():
            correct += 1

    accuracy = correct / len(questions)
    print(f"Eval Accuracy: {accuracy * 100}%")
    assert accuracy > 0.0

if __name__ == "__main__":
    if wait_for_gateway():
        test_ingest_and_query()
