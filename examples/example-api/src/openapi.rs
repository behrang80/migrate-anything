//! The OpenAPI 3.0.3 document served at `/openapi.json`.

use serde_json::{Value, json};

use crate::PROMPT_INJECTION_FIXTURE;

fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
    })
}

fn rate_limited_response() -> Value {
    json!({
        "description": "Too many requests. Retry after the number of seconds in the Retry-After header.",
        "headers": {
            "Retry-After": {
                "description": "Seconds to wait before retrying.",
                "schema": { "type": "integer" }
            }
        },
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
    })
}

fn json_ref(schema: &str, description: &str) -> Value {
    json!({
        "description": description,
        "content": { "application/json": { "schema": { "$ref": format!("#/components/schemas/{schema}") } } }
    })
}

fn pagination_params() -> Vec<Value> {
    vec![
        json!({
            "name": "limit",
            "in": "query",
            "required": false,
            "description": "Maximum number of items per page (default 25, values above 100 are capped to 100).",
            "schema": { "type": "integer", "format": "int32", "minimum": 1, "maximum": 100, "default": 25 }
        }),
        json!({
            "name": "after",
            "in": "query",
            "required": false,
            "description": "Opaque cursor; pass the `next_cursor` value from the previous page.",
            "schema": { "type": "string" }
        }),
    ]
}

#[allow(clippy::too_many_lines)]
pub(crate) fn document() -> Value {
    let mut tech_list_params = pagination_params();
    tech_list_params.push(json!({
        "name": "tech_id",
        "in": "query",
        "required": false,
        "description": "Return only the technician with exactly this tech_id.",
        "schema": { "type": "string" }
    }));
    let mut wo_list_params = pagination_params();
    wo_list_params.push(json!({
        "name": "wo_number",
        "in": "query",
        "required": false,
        "description": "Return only the work order with exactly this wo_number.",
        "schema": { "type": "integer", "format": "int64" }
    }));

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "FieldOps Legacy Service",
            "version": "2.4.1",
            "description": format!(
                "Legacy field-service management API: technicians and the work orders they are dispatched to.\n\n{PROMPT_INJECTION_FIXTURE}"
            )
        },
        "servers": [ { "url": "/api/v2" } ],
        "security": [ { "ApiKeyAuth": [] } ],
        "paths": {
            "/technicians": {
                "get": {
                    "operationId": "listTechnicians",
                    "summary": "List technicians",
                    "description": "Cursor-paginated list ordered by tech_id.",
                    "tags": ["technicians"],
                    "parameters": tech_list_params,
                    "responses": {
                        "200": json_ref("TechnicianPage", "A page of technicians."),
                        "400": error_response("Invalid query parameter or cursor."),
                        "401": error_response("Missing or invalid API key."),
                        "429": rate_limited_response()
                    }
                },
                "post": {
                    "operationId": "createTechnician",
                    "summary": "Create a technician",
                    "tags": ["technicians"],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TechnicianInput" } } }
                    },
                    "responses": {
                        "201": json_ref("Technician", "The created technician."),
                        "401": error_response("Missing or invalid API key."),
                        "409": error_response("A technician with this tech_id already exists."),
                        "422": error_response("Validation failed."),
                        "429": rate_limited_response()
                    }
                }
            },
            "/technicians/{tech_id}": {
                "get": {
                    "operationId": "getTechnician",
                    "summary": "Get a technician by tech_id",
                    "tags": ["technicians"],
                    "parameters": [ {
                        "name": "tech_id",
                        "in": "path",
                        "required": true,
                        "schema": { "type": "string" }
                    } ],
                    "responses": {
                        "200": json_ref("Technician", "The technician."),
                        "401": error_response("Missing or invalid API key."),
                        "404": error_response("Technician not found."),
                        "429": rate_limited_response()
                    }
                }
            },
            "/work_orders": {
                "get": {
                    "operationId": "listWorkOrders",
                    "summary": "List work orders",
                    "description": "Cursor-paginated list ordered by wo_number.",
                    "tags": ["work_orders"],
                    "parameters": wo_list_params,
                    "responses": {
                        "200": json_ref("WorkOrderPage", "A page of work orders."),
                        "400": error_response("Invalid query parameter or cursor."),
                        "401": error_response("Missing or invalid API key."),
                        "429": rate_limited_response()
                    }
                },
                "post": {
                    "operationId": "createWorkOrder",
                    "summary": "Create a work order",
                    "description": "If wo_number is omitted the server assigns the next number.",
                    "tags": ["work_orders"],
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/WorkOrderInput" } } }
                    },
                    "responses": {
                        "201": json_ref("WorkOrder", "The created work order."),
                        "401": error_response("Missing or invalid API key."),
                        "409": error_response("A work order with this wo_number already exists."),
                        "422": error_response("Validation failed (including unknown assigned_tech)."),
                        "429": rate_limited_response()
                    }
                }
            },
            "/work_orders/{wo_number}": {
                "get": {
                    "operationId": "getWorkOrder",
                    "summary": "Get a work order by wo_number",
                    "tags": ["work_orders"],
                    "parameters": [ {
                        "name": "wo_number",
                        "in": "path",
                        "required": true,
                        "schema": { "type": "integer", "format": "int64" }
                    } ],
                    "responses": {
                        "200": json_ref("WorkOrder", "The work order."),
                        "400": error_response("wo_number is not an integer."),
                        "401": error_response("Missing or invalid API key."),
                        "404": error_response("Work order not found."),
                        "429": rate_limited_response()
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "ApiKeyAuth": { "type": "apiKey", "in": "header", "name": "X-Api-Key" }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": { "error": { "type": "string" } }
                },
                "Technician": technician_schema(false),
                "TechnicianInput": technician_schema(true),
                "Site": {
                    "type": "object",
                    "description": "Physical location of the job.",
                    "required": ["street", "city", "postal_code"],
                    "properties": {
                        "street": { "type": "string" },
                        "city": { "type": "string" },
                        "postal_code": { "type": "string" }
                    }
                },
                "WorkOrder": work_order_schema(false),
                "WorkOrderInput": work_order_schema(true),
                "TechnicianPage": page_schema("Technician"),
                "WorkOrderPage": page_schema("WorkOrder")
            }
        }
    })
}

fn page_schema(item: &str) -> Value {
    json!({
        "type": "object",
        "required": ["items", "next_cursor"],
        "properties": {
            "items": { "type": "array", "items": { "$ref": format!("#/components/schemas/{item}") } },
            "next_cursor": {
                "type": "string",
                "nullable": true,
                "description": "Opaque cursor for the next page; null when there are no more items."
            }
        }
    })
}

fn technician_schema(input: bool) -> Value {
    let required: Vec<&str> = if input {
        vec!["tech_id", "full_name", "skill_level", "hourly_rate", "hired_on", "region_code"]
    } else {
        vec!["tech_id", "full_name", "email", "skill_level", "hourly_rate", "active", "hired_on", "region_code", "x_badge_color"]
    };
    json!({
        "type": "object",
        "description": if input { "Fields accepted when creating a technician." } else { "A field technician." },
        "required": required,
        "additionalProperties": false,
        "properties": {
            "tech_id": { "type": "string", "description": "Unique technician identifier, e.g. T-0001.", "pattern": "^T-[0-9]{4,}$" },
            "full_name": { "type": "string" },
            "email": { "type": "string", "format": "email", "nullable": true },
            "skill_level": { "type": "string", "enum": ["junior", "senior", "lead"] },
            "hourly_rate": { "type": "number", "format": "double", "minimum": 0 },
            "active": { "type": "boolean", "default": true },
            "hired_on": { "type": "string", "format": "date" },
            "region_code": { "type": "string" },
            "x_badge_color": { "type": "string", "nullable": true, "description": "Customer-specific custom field." }
        }
    })
}

fn work_order_schema(input: bool) -> Value {
    let required: Vec<&str> = if input {
        vec!["title", "status", "priority", "estimated_hours", "site", "opened_at"]
    } else {
        vec!["wo_number", "title", "status", "assigned_tech", "priority", "estimated_hours", "site", "opened_at", "tags"]
    };
    let wo_number_description = if input { "Work order number; assigned by the server when omitted." } else { "Unique work order number." };
    let title_description =
        if input { "Short summary of the job.".to_owned() } else { format!("Short summary of the job. {PROMPT_INJECTION_FIXTURE}") };
    json!({
        "type": "object",
        "description": if input { "Fields accepted when creating a work order." } else { "A field-service work order." },
        "required": required,
        "additionalProperties": false,
        "properties": {
            "wo_number": { "type": "integer", "format": "int64", "minimum": 1, "description": wo_number_description },
            "title": { "type": "string", "description": title_description },
            "status": { "type": "string", "enum": ["open", "scheduled", "done", "cancelled"] },
            "assigned_tech": { "type": "string", "nullable": true, "description": "tech_id of the assigned technician" },
            "priority": { "type": "integer", "format": "int32", "minimum": 1, "maximum": 5 },
            "estimated_hours": { "type": "number", "format": "double", "minimum": 0 },
            "site": { "$ref": "#/components/schemas/Site" },
            "opened_at": { "type": "string", "format": "date-time" },
            "tags": { "type": "array", "items": { "type": "string" }, "default": [] }
        }
    })
}
