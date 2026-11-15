"""Recover the verified CRX publisher key for Chromium's unpacked loader."""
import base64, hashlib, pathlib, subprocess, tempfile

def fields(data):
    at=0
    def varint():
        nonlocal at
        value=0
        for shift in range(0,70,7):
            if at>=len(data): raise ValueError('truncated protobuf')
            byte=data[at];at+=1;value|=(byte&127)<<shift
            if byte<128:return value
        raise ValueError('oversized protobuf integer')
    while at<len(data):
        tag=varint();wire=tag&7;number=tag>>3
        if not number:raise ValueError('zero field')
        if wire==0:value=varint()
        elif wire in (1,2,5):
            size=varint() if wire==2 else (8 if wire==1 else 4)
            if size>len(data)-at:raise ValueError('truncated protobuf field')
            value=data[at:at+size];at+=size
        else:raise ValueError('unsupported protobuf field')
        yield number,wire,value

def identity_key(data,identity):
    if data[:8]!=b'Cr24\x03\x00\x00\x00' or len(data)<12:raise ValueError('expected CRX3')
    size=int.from_bytes(data[8:12],'little')
    if size>65536 or 12+size>len(data):raise ValueError('invalid CRX header')
    header=list(fields(data[12:12+size]));signed=[v for n,w,v in header if n==10000 and w==2]
    if len(signed)!=1:raise ValueError('missing signed header')
    payload_ids=[v for n,w,v in fields(signed[0]) if n==1 and w==2]
    def chrome_id(digest):return ''.join(chr(97+v) for b in digest for v in (b>>4,b&15))
    if len(payload_ids)!=1 or chrome_id(payload_ids[0])!=identity:raise ValueError('signed CRX identity mismatch')
    for number,wire,proof in header:
        if number not in (2,3) or wire!=2:continue
        proof=dict((n,v) for n,w,v in fields(proof) if w==2)
        key=proof.get(1);signature=proof.get(2)
        if not key or not signature or chrome_id(hashlib.sha256(key).digest()[:16])!=identity:continue
        with tempfile.TemporaryDirectory(prefix='crx-public-proof-') as tmp:
            root=pathlib.Path(tmp)
            (root/'key.der').write_bytes(key);(root/'signature').write_bytes(signature)
            (root/'message').write_bytes(b'CRX3 SignedData\x00'+len(signed[0]).to_bytes(4,'little')+signed[0]+data[12+size:])
            subprocess.run(['openssl','pkey','-pubin','-inform','DER','-in',str(root/'key.der'),'-out',str(root/'key.pem')],check=True,capture_output=True,timeout=10)
            subprocess.run(['openssl','dgst','-sha256','-verify',str(root/'key.pem'),'-signature',str(root/'signature'),str(root/'message')],check=True,capture_output=True,timeout=30)
        return base64.b64encode(key).decode()
    raise ValueError('no verified publisher proof for pinned extension identity')
