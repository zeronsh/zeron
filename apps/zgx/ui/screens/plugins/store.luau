--!strict
-- The store's plugins, as Settings → Plugins asks for them
-- (screens/plugins/store_client).

local client = require("@app/screens/plugins/store_client")

export type Badge = client.Badge
export type Author = client.Author
export type Card = client.Card
export type Version = client.Version
export type Query = client.Query
export type Scope = client.Scope

return client.make("plugins.store", "store")
