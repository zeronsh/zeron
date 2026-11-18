-- The audit log read for one plugin (`GET /store/v1/admin/audit?plugin=`):
-- its page and its total (amendment 38) by index, not by reading the whole
-- log.
CREATE INDEX audit_plugin ON audit (plugin_id, seq);
