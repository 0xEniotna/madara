POST http://0.0.0.0:9933
Content-Type: application/json
{
    "jsonrpc": "2.0",
    "method": "starknet_syncing",
    "params": [],
    "id":1
}

HTTP 200
[Asserts]
jsonpath "$.error" not exists
jsonpath "$.result" exists
