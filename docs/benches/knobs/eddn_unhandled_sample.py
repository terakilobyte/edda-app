import collections, json, sys, time, zlib, zmq
secs=int(sys.argv[1]); out=sys.argv[2]
ctx=zmq.Context(); s=ctx.socket(zmq.SUB); s.setsockopt(zmq.SUBSCRIBE,b""); s.setsockopt(zmq.RCVTIMEO,5000); s.connect("tcp://eddn.edcd.io:9500")
schemas=collections.Counter(); events=collections.Counter(); keys=collections.defaultdict(collections.Counter); examples={}
n=0; t0=time.time()
while time.time()-t0<secs:
    try: raw=s.recv()
    except zmq.Again: continue
    n+=1
    try: m=json.loads(zlib.decompress(raw))
    except Exception: schemas["<undecodable>"]+=1; continue
    sch=m.get("$schemaRef","?").rsplit("/schemas/",1)[-1]; msg=m.get("message",{})
    schemas[sch]+=1
    kind=sch
    if sch.startswith("journal/"):
        ev=msg.get("event","?"); events[ev]+=1; kind="journal:"+ev
    for k in msg: keys[kind][k]+=1
    if kind not in examples:
        ex={k:(v if not isinstance(v,(list,dict)) else ("[%d items]"%len(v) if isinstance(v,list) else "{%s}"%",".join(list(v)[:8]))) for k,v in msg.items()}
        examples[kind]=ex
json.dump({"frames":n,"seconds":secs,"start":time.strftime("%Y-%m-%dT%H:%M:%SZ",time.gmtime(t0)),"schemas":schemas,"journal_events":events,"keys":{k:dict(v) for k,v in keys.items()},"examples":examples},open(out,"w"),indent=1)
print("frames",n)
