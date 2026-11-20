"""Role mapping and name normalization from the frozen non-CSS census helper."""
import re
DEFAULTS={'http':[8000,8001],'http-local':[8002],'http-public':[8003],'https':[8443,8444],'https-local':[8445],'https-public':[8446],'ws':[8888],'wss':[8889],'h2':[9000],'webtransport-h3':[11000],'dns':[8053]}
# An encodeURIComponent() URL (including nested encoding). Preserve casing and escape depth.
ENC=re.compile(r'(?P<scheme>https?|wss?)%(?P<depth>(?:25)*)3a%(?P=depth)2f%(?P=depth)2f(?P<host>(?:[a-z0-9_-]+\.)*(?:not-)?web-platform\.test)%(?P=depth)3a(?P<port>[0-9]+)(?![a-z0-9.-])',re.I)
def canonical_name(name,mapping):
    if not mapping or '%' not in name:return name
    def sub(m):
        if m.start() and name[m.start()-1].isascii() and (name[m.start()-1].isalnum() or name[m.start()-1] in '+.-'):
            # A nested URL may begin immediately after an escaped query delimiter.
            if not re.search(r'%(?:25)*(?:3d|26|2f|23|3f|20|22|27|3a|2c)$',name[:m.start()],re.I):return m[0]
        match=mapping.get((m['scheme'].lower(),int(m['port'])))
        if match is None:return m[0]
        return m[0][:m.start('port')-m.start()]+str(match)
    return ENC.sub(sub,name)
