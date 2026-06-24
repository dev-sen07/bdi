// ============================================================================
// consolidation.rs — Algoritmo de consolidación semántica (contribución tesis)
// ============================================================================
//
// Backend: Neo4j (vía neo4rs) con un índice vectorial sobre la propiedad
// `embedding` de los nodos :Fact. Cada hecho entrante NO se inserta a ciegas:
// el motor lo compara contra el vecindario semántico y decide qué hacer.
//
// Diferencias clave frente a una inserción ingenua ("siempre crear nodo"):
//   1. MERGE real: cuando el hecho casi coincide con uno existente, se FUSIONAN
//      (promedio ponderado de embeddings + fusión de contenido), no se duplica.
//   2. Recuperación TOP-K (no solo el vecino más cercano): permite relacionar el
//      hecho nuevo con TODOS los nodos del rango "relacionado", produciendo un
//      grafo con estructura más rica.
//   3. DISCARDED: un duplicado exacto no aporta información nueva; se refuerza
//      el uso del nodo existente pero no se crea ni modifica estructura.
//
// Nota sobre los scores: el índice vectorial de Neo4j con `cosine` devuelve un
// score normalizado a (0,1] donde score = (1 + cos)/2. Los umbrales de abajo se
// interpretan sobre ESA escala normalizada (no sobre el coseno crudo), por lo
// que un umbral de 0.92 exige una coincidencia muy fuerte.
// ============================================================================

use neo4rs::Graph;
use uuid::Uuid;

// ----------------------------------------------------------------------------
// Parámetros adaptativos.
// Se leen de variables de entorno (para poder tunear en los experimentos de la
// tesis sin recompilar) y caen a un valor por defecto razonable si no existen.
// ----------------------------------------------------------------------------
const DEFAULT_THRESHOLD_MERGE: f32 = 0.92; // >= esto -> fusionar (casi idéntico)
const DEFAULT_THRESHOLD_RELATE: f32 = 0.75; // >= esto -> relacionar (relacionado)
const DEFAULT_THRESHOLD_DUPLICATE: f32 = 0.985; // >= esto + texto igual -> descartar

/// Lee un umbral desde una variable de entorno o usa el valor por defecto.
fn env_threshold(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(default)
}

/// Resultado de consolidar un hecho entrante.
pub enum ConsolidationResult {
    /// Fusionado con un nodo casi idéntico (se promedió el embedding).
    Merged { node_id: String },
    /// Insertado como nodo nuevo y enlazado a uno o más nodos relacionados.
    Related { new_id: String, related_to: String },
    /// Insertado como nodo completamente nuevo (sin vecinos relevantes).
    Inserted { node_id: String },
    /// Descartado: duplicado exacto; solo se reforzó el uso del nodo existente.
    Discarded { node_id: String },
}

/// Candidato recuperado del índice vectorial.
struct Candidate {
    id: String,
    content: String,
    embedding: Vec<f32>,
    uso_count: i64,
    score: f32,
}

/// Normaliza texto para comparar duplicados: minúsculas, recorta y colapsa
/// espacios. Evita marcar como "distintos" hechos que solo cambian en formato.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Promedio PONDERADO por uso entre el embedding almacenado y el entrante.
///
/// Diseño: un nodo con uso_count alto representa evidencia acumulada de muchas
/// observaciones; su centroide no debe moverse tanto por un solo hecho nuevo.
/// new[i] = (uso * old[i] + incoming[i]) / (uso + 1)
fn weighted_average(old: &[f32], incoming: &[f32], uso_count: i64) -> Vec<f32> {
    let w = uso_count.max(1) as f32;
    old.iter()
        .zip(incoming.iter())
        .map(|(o, n)| (w * o + n) / (w + 1.0))
        .collect()
}

/// Recupera los top-k vecinos del índice vectorial, ya materializados en memoria
/// (consumimos el stream antes de hacer escrituras para no mantener abierto el
/// cursor mientras mutamos el grafo sobre la misma conexión).
async fn top_k_candidates(
    graph: &Graph,
    embedding: &[f32],
    k: i64,
) -> Result<Vec<Candidate>, Box<dyn std::error::Error>> {
    let cypher = "
        CALL db.index.vector.queryNodes('fact_embeddings', $k, $embedding)
        YIELD node, score
        RETURN node.id        AS id,
               node.content    AS content,
               node.embedding  AS embedding,
               coalesce(node.uso_count, 1) AS uso_count,
               score
        ORDER BY score DESC
    ";

    let mut result = graph
        .execute(
            neo4rs::query(cypher)
                .param("k", k)
                .param("embedding", embedding.to_vec()),
        )
        .await?;

    let mut candidates = Vec::new();
    while let Ok(Some(row)) = result.next().await {
        // El embedding viene como lista de f64 desde Neo4j; lo bajamos a f32.
        let emb: Vec<f32> = row
            .get::<Vec<f64>>("embedding")
            .unwrap_or_default()
            .into_iter()
            .map(|v| v as f32)
            .collect();

        candidates.push(Candidate {
            id: row.get::<String>("id").unwrap_or_default(),
            content: row.get::<String>("content").unwrap_or_default(),
            embedding: emb,
            uso_count: row.get::<i64>("uso_count").unwrap_or(1),
            score: row.get::<f64>("score").unwrap_or(0.0) as f32,
        });
    }

    Ok(candidates)
}

/// Algoritmo principal de consolidación.
///
/// Flujo de decisión sobre el vecino MÁS cercano (best):
///   - best.score >= DUPLICATE  y texto idéntico  -> DISCARDED (refuerza uso)
///   - best.score >= MERGE                          -> MERGED  (fusiona embedding+contenido)
///   - best.score >= RELATE                         -> RELATED (nuevo nodo + aristas)
///   - en otro caso                                  -> INSERTED (nodo aislado nuevo)
pub async fn consolidate(
    incoming_text: &str,
    incoming_embedding: &[f32],
    graph: &Graph,
) -> Result<ConsolidationResult, Box<dyn std::error::Error>> {
    let threshold_merge = env_threshold("THRESHOLD_MERGE", DEFAULT_THRESHOLD_MERGE);
    let threshold_relate = env_threshold("THRESHOLD_RELATE", DEFAULT_THRESHOLD_RELATE);
    let threshold_duplicate = env_threshold("THRESHOLD_DUPLICATE", DEFAULT_THRESHOLD_DUPLICATE);

    // 1. Recuperar el vecindario semántico (top-5 según la spec de la tesis).
    let candidates = top_k_candidates(graph, incoming_embedding, 5).await?;

    // 2. Sin vecinos: el grafo está vacío o no hay nada parecido -> nodo nuevo.
    let best = match candidates.first() {
        Some(c) => c,
        None => return insert_new(graph, incoming_text, incoming_embedding).await,
    };

    // 3a. DUPLICADO EXACTO: muy similar Y el texto normalizado coincide.
    //     No aporta información nueva: solo reforzamos el uso del nodo existente.
    if best.score >= threshold_duplicate && normalize(&best.content) == normalize(incoming_text) {
        reinforce_usage(graph, &best.id).await?;
        return Ok(ConsolidationResult::Discarded {
            node_id: best.id.clone(),
        });
    }

    // 3b. MERGE: casi idéntico pero con matices -> fusionar de verdad.
    if best.score >= threshold_merge {
        return merge_into(graph, best, incoming_text, incoming_embedding).await;
    }

    // 3c. RELATE: relacionado (no idéntico). Insertamos un nodo nuevo y lo
    //     enlazamos con TODOS los candidatos dentro del rango "relacionado",
    //     no solo con el más cercano -> grafo más rico semánticamente.
    if best.score >= threshold_relate {
        let new_id = Uuid::new_v4().to_string();
        create_node(graph, &new_id, incoming_text, incoming_embedding).await?;

        for c in candidates.iter().filter(|c| c.score >= threshold_relate) {
            create_related_edge(graph, &new_id, &c.id, c.score).await?;
        }

        return Ok(ConsolidationResult::Related {
            new_id,
            related_to: best.id.clone(),
        });
    }

    // 4. Nada lo bastante similar -> nodo completamente nuevo.
    insert_new(graph, incoming_text, incoming_embedding).await
}

// ----------------------------------------------------------------------------
// Operaciones atómicas sobre el grafo (cada una es una sola consulta Cypher).
// ----------------------------------------------------------------------------

/// Inserta un nodo :Fact nuevo y devuelve el resultado `Inserted`.
async fn insert_new(
    graph: &Graph,
    text: &str,
    embedding: &[f32],
) -> Result<ConsolidationResult, Box<dyn std::error::Error>> {
    let new_id = Uuid::new_v4().to_string();
    create_node(graph, &new_id, text, embedding).await?;
    Ok(ConsolidationResult::Inserted { node_id: new_id })
}

/// Crea un nodo :Fact con los metadatos de uso/tiempo inicializados.
async fn create_node(
    graph: &Graph,
    id: &str,
    text: &str,
    embedding: &[f32],
) -> Result<(), Box<dyn std::error::Error>> {
    graph
        .run(
            neo4rs::query(
                "CREATE (n:Fact {
                    id: $id,
                    content: $content,
                    embedding: $embedding,
                    uso_count: 1,
                    merge_count: 0,
                    created_at: timestamp(),
                    last_accessed: timestamp()
                 })",
            )
            .param("id", id.to_string())
            .param("content", text.to_string())
            .param("embedding", embedding.to_vec()),
        )
        .await?;
    Ok(())
}

/// Crea (o refuerza) una arista RELATED dirigida new -> target, guardando el
/// score de similitud como peso. MERGE evita aristas duplicadas si el par ya
/// existía, y nos quedamos con el mayor score observado.
async fn create_related_edge(
    graph: &Graph,
    new_id: &str,
    target_id: &str,
    score: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    graph
        .run(
            neo4rs::query(
                "MATCH (a:Fact {id: $new_id}), (b:Fact {id: $target_id})
                 MERGE (a)-[r:RELATED]->(b)
                 ON CREATE SET r.score = $score, r.created_at = timestamp()
                 ON MATCH  SET r.score = CASE WHEN $score > r.score THEN $score ELSE r.score END",
            )
            .param("new_id", new_id.to_string())
            .param("target_id", target_id.to_string())
            .param("score", score as f64),
        )
        .await?;
    Ok(())
}

/// Refuerza el uso de un nodo (duplicado exacto): +1 uso y actualiza acceso.
async fn reinforce_usage(graph: &Graph, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    graph
        .run(
            neo4rs::query(
                "MATCH (n:Fact {id: $id})
                 SET n.uso_count = coalesce(n.uso_count, 1) + 1,
                     n.last_accessed = timestamp()",
            )
            .param("id", id.to_string()),
        )
        .await?;
    Ok(())
}

/// MERGE real: promedia (ponderado) el embedding, fusiona el contenido si el
/// hecho entrante aporta texto nuevo, y actualiza los contadores. El nodo
/// resultante conserva su id (la "memoria" se refuerza, no se duplica).
async fn merge_into(
    graph: &Graph,
    target: &Candidate,
    incoming_text: &str,
    incoming_embedding: &[f32],
) -> Result<ConsolidationResult, Box<dyn std::error::Error>> {
    // 1. Nuevo centroide del concepto: promedio ponderado por uso.
    let merged_embedding =
        weighted_average(&target.embedding, incoming_embedding, target.uso_count);

    // 2. Fusión de contenido: solo añadimos el texto entrante si NO está ya
    //    contenido en el del nodo (evita crecer sin límite con repeticiones).
    let merged_content = if normalize(&target.content).contains(&normalize(incoming_text)) {
        target.content.clone()
    } else {
        format!("{} | {}", target.content, incoming_text)
    };

    graph
        .run(
            neo4rs::query(
                "MATCH (n:Fact {id: $id})
                 SET n.content = $content,
                     n.embedding = $embedding,
                     n.uso_count = coalesce(n.uso_count, 1) + 1,
                     n.merge_count = coalesce(n.merge_count, 0) + 1,
                     n.last_accessed = timestamp()",
            )
            .param("id", target.id.clone())
            .param("content", merged_content)
            .param("embedding", merged_embedding),
        )
        .await?;

    Ok(ConsolidationResult::Merged {
        node_id: target.id.clone(),
    })
}
