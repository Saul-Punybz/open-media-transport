import sys
seen={}
for line in open(sys.argv[1]):
    t,sp,dp,pl=line.rstrip('\n').split('\t')
    b=bytes.fromhex(pl.replace(':',''))
    i=0
    while i+16<=len(b):
        ln=int.from_bytes(b[i+12:i+16],'little')
        x=b[i+16:i+16+ln]
        if b[i+1]==1 and b'Redirect' in x: print(f"{float(t):6.2f} {sp}->{dp} ts={int.from_bytes(b[i+2:i+10],'little')} len={ln} {x!r}")
        i+=16+ln
