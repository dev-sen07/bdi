mod consolidation;

use tonic::{transport::Server, Request, Response, Status};
use std::sync::Arc;
use neo4rs::{Graph, ConfigBuilder};

pub mod bdi {
    tonic::include_proto!("bdi");
}

use bdi::graph_engine_server::{GraphEngine, GraphEngineServer};
use bdi::{
    QueryRequest, QueryResponse, NodeResult,
    IngestRequest, IngestResponse,
    StatsRequest, StatsResponse,
    ListRequest, ListResponse, NodeInfo,
    UpdateRequest, UpdateResponse,
    DeleteRequest, DeleteResponse,
};

#[derive(Clone)]
pub struct MyGraphEngine {
    neo4j: Arc<Graph>,
}

#[tonic::async_trait]
impl GraphEngine for MyGraphEngine {
    async fn query(&self, request: Request<QueryRequest>) -> Result<Response<QueryResponse>, Status> {
        let req = request.into_inner();
        let top_k = if req.top_k > 0 { req.top_k } else { 5 };
        
        let query_str = format!("CALL db.index.vector.queryNodes('fact_embeddings', {}, $embedding) YIELD node, score RETURN node.id AS id, node.content AS content, score", top_k);
        
        let mut result = self.neo4j.execute(
            neo4rs::query(&query_str)
                .param("embedding", req.embedding.clone())
        ).await.map_err(|e| Status::internal(e.to_string()))?;
        
        let mut results = Vec::new();
        while let Ok(Some(row)) = result.next().await {
            let id = row.get::<String>("id").unwrap_or_default();
            let content = row.get::<String>("content").unwrap_or_default();
            let score = row.get::<f64>("score").unwrap_or(0.0) as f32;
            
            results.push(NodeResult {
                id: id.clone(),
                content,
                score,
            });

            let update_q = neo4rs::query("MATCH (n:Fact {id: $id}) SET n.uso_count = coalesce(n.uso_count, 1) + 1, n.last_accessed = timestamp()")
                .param("id", id);
            let _ = self.neo4j.run(update_q).await;
        }

        Ok(Response::new(QueryResponse { results }))
    }

    async fn ingest(&self, request: Request<IngestRequest>) -> Result<Response<IngestResponse>, Status> {
        let req = request.into_inner();
        
        let result = consolidation::consolidate(&req.text, &req.embedding, &self.neo4j).await
            .map_err(|e| Status::internal(e.to_string()))?;
        
        let (action, node_id) = match result {
            consolidation::ConsolidationResult::Merged { node_id } => ("merged", node_id),
            consolidation::ConsolidationResult::Related { new_id, .. } => ("related", new_id),
            consolidation::ConsolidationResult::Inserted { node_id } => ("inserted", node_id),
            consolidation::ConsolidationResult::Discarded { node_id } => ("discarded", node_id),
        };

        Ok(Response::new(IngestResponse {
            action: action.to_string(),
            node_id,
        }))
    }

    async fn get_stats(&self, _request: Request<StatsRequest>) -> Result<Response<StatsResponse>, Status> {
        let mut res = self.neo4j.execute(neo4rs::query("MATCH (n:Fact) RETURN count(n) as total_nodes")).await.map_err(|e| Status::internal(e.to_string()))?;
        let mut total_nodes = 0;
        if let Ok(Some(row)) = res.next().await {
            total_nodes = row.get::<i64>("total_nodes").unwrap_or(0) as i32;
        }
        
        let mut res = self.neo4j.execute(neo4rs::query("MATCH ()-[r]->() RETURN count(r) as total_edges")).await.map_err(|e| Status::internal(e.to_string()))?;
        let mut total_edges = 0;
        if let Ok(Some(row)) = res.next().await {
            total_edges = row.get::<i64>("total_edges").unwrap_or(0) as i32;
        }

        Ok(Response::new(StatsResponse {
            total_nodes,
            total_edges,
            forgotten_nodes: 0,
        }))
    }

    // ------------------------------------------------------------------------
    // ListNodes: lista paginada para administrar el conocimiento desde la UI.
    // Devuelve metadatos de uso/tiempo y el grado (número de conexiones) de cada
    // nodo, ordenados del más recientemente usado al menos. Nunca enviamos el
    // embedding (es ruido para la UI y aumenta mucho el tamaño de la respuesta).
    // ------------------------------------------------------------------------
    async fn list_nodes(
        &self,
        request: Request<ListRequest>,
    ) -> Result<Response<ListResponse>, Status> {
        let req = request.into_inner();
        let limit = if req.limit > 0 { req.limit } else { 50 } as i64;
        let offset = req.offset.max(0) as i64;

        let cypher = "
            MATCH (n:Fact)
            RETURN n.id AS id,
                   n.content AS content,
                   coalesce(n.uso_count, 1)   AS uso_count,
                   coalesce(n.merge_count, 0) AS merge_count,
                   coalesce(n.created_at, 0)  AS created_at,
                   coalesce(n.last_accessed, 0) AS last_accessed,
                   COUNT { (n)--() } AS degree
            ORDER BY n.last_accessed DESC
            SKIP $offset LIMIT $limit
        ";

        let mut result = self
            .neo4j
            .execute(
                neo4rs::query(cypher)
                    .param("offset", offset)
                    .param("limit", limit),
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut nodes = Vec::new();
        while let Ok(Some(row)) = result.next().await {
            nodes.push(NodeInfo {
                id: row.get::<String>("id").unwrap_or_default(),
                content: row.get::<String>("content").unwrap_or_default(),
                uso_count: row.get::<i64>("uso_count").unwrap_or(1) as i32,
                merge_count: row.get::<i64>("merge_count").unwrap_or(0) as i32,
                created_at: row.get::<i64>("created_at").unwrap_or(0),
                last_accessed: row.get::<i64>("last_accessed").unwrap_or(0),
                degree: row.get::<i64>("degree").unwrap_or(0) as i32,
            });
        }

        // Total global para que la UI pueda paginar.
        let mut total = 0;
        let mut res = self
            .neo4j
            .execute(neo4rs::query("MATCH (n:Fact) RETURN count(n) AS total"))
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        if let Ok(Some(row)) = res.next().await {
            total = row.get::<i64>("total").unwrap_or(0) as i32;
        }

        Ok(Response::new(ListResponse { nodes, total }))
    }

    // ------------------------------------------------------------------------
    // UpdateNode: edita el contenido de un nodo. El gateway ya recalculó el
    // embedding a partir del nuevo texto, así que actualizamos AMBOS para que el
    // índice vectorial siga siendo consistente con el contenido visible.
    // ------------------------------------------------------------------------
    async fn update_node(
        &self,
        request: Request<UpdateRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let req = request.into_inner();

        let cypher = "
            MATCH (n:Fact {id: $id})
            SET n.content = $content,
                n.embedding = $embedding,
                n.last_accessed = timestamp()
            RETURN n.id AS id
        ";

        let mut result = self
            .neo4j
            .execute(
                neo4rs::query(cypher)
                    .param("id", req.id.clone())
                    .param("content", req.text)
                    .param("embedding", req.embedding),
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Si no hubo fila de retorno, el id no existía.
        let success = matches!(result.next().await, Ok(Some(_)));

        Ok(Response::new(UpdateResponse {
            success,
            id: req.id,
        }))
    }

    // ------------------------------------------------------------------------
    // DeleteNode: borra el nodo y todas sus aristas. DETACH DELETE las elimina
    // en una sola operación; primero contamos el grado para informarlo a la UI.
    // Al borrar la propiedad `embedding` el índice vectorial se actualiza solo.
    // ------------------------------------------------------------------------
    async fn delete_node(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let req = request.into_inner();

        let cypher = "
            MATCH (n:Fact {id: $id})
            WITH n, COUNT { (n)--() } AS deleted_edges
            DETACH DELETE n
            RETURN deleted_edges
        ";

        let mut result = self
            .neo4j
            .execute(neo4rs::query(cypher).param("id", req.id))
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let (success, deleted_edges) = match result.next().await {
            Ok(Some(row)) => (true, row.get::<i64>("deleted_edges").unwrap_or(0) as i32),
            _ => (false, 0),
        };

        Ok(Response::new(DeleteResponse {
            success,
            deleted_edges,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50052".parse()?;
    
    // Connect to Neo4j
    let neo4j_uri = std::env::var("NEO4J_URI").unwrap_or_else(|_| "127.0.0.1:7687".to_string());
    println!("Connecting to Neo4j at {}...", neo4j_uri);
    
    let config = ConfigBuilder::default()
        .uri(&neo4j_uri)
        .user("neo4j") 
        .password("password")
        .build()?;
        
    let mut graph = None;
    for i in 1..=10 {
        match Graph::connect(config.clone()).await {
            Ok(g) => {
                println!("Connected to Neo4j successfully!");
                
                // Intentar crear el index
                let create_idx = neo4rs::query("
                    CREATE VECTOR INDEX fact_embeddings IF NOT EXISTS 
                    FOR (n:Fact) ON (n.embedding) 
                    OPTIONS {indexConfig: {`vector.dimensions`: 384, `vector.similarity_function`: 'cosine'}}
                ");
                
                match g.run(create_idx).await {
                    Ok(_) => {
                        println!("Vector index created or already exists.");
                        graph = Some(g);
                        break;
                    },
                    Err(e) => println!("Connection ok, but query failed (Neo4j starting?): {}", e),
                }
            },
            Err(e) => println!("Attempt {} to connect to Neo4j failed: {}", i, e),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }

    let graph = graph.expect("Failed to connect and initialize Neo4j after 10 attempts");

    let engine = MyGraphEngine {
        neo4j: Arc::new(graph),
    };

    println!("GraphEngine listening on {}", addr);

    Server::builder()
        .add_service(GraphEngineServer::new(engine))
        .serve(addr)
        .await?;

    Ok(())
}
