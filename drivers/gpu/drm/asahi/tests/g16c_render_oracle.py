# SPDX-License-Identifier: MIT
"""CPU RGBA8 oracle for the source fixtures' POT Morton surface layout.

Addressing follows Mesa src/asahi/layout/layout.c::ail_initialize_twiddled and
tests/test-twiddling.cpp::tiled_offset_el. Multisample records contain one RGBA8
word per sample; writable array layers are 16-KiB aligned. No GPU output input.
"""
def spread(value):
    result=0
    for bit in range(value.bit_length()):
        result|=((value>>bit)&1)<<(2*bit)
    return result

def rgba8_expected(seed,width,height,layers,samples,color,offset=0):
    assert width>0 and height>0 and layers>0 and samples in (1,2,4)
    assert color in ('red','three-layer-colors','two-layer-red-green')
    if color=='three-layer-colors':
        assert layers==3  # The particular source shader's draw count.
    if color=='two-layer-red-green':
        assert layers==2
    pw=1<<(width-1).bit_length()
    ph=1<<(height-1).bit_length()
    tile=min(pw,ph)
    layer_stride=(pw*ph*4*samples+0x3fff)&-0x4000
    expected=bytearray(seed)
    points=set()
    for layer in range(layers):
        pixel=bytes.fromhex('ff0000ff') if color=='red' else bytes(((17+37*layer)%256,(51+73*layer)%256,(137+29*layer)%256,255))
        if color=='two-layer-red-green':
            pixel=bytes.fromhex('ff0000ff' if layer==0 else '00ff00ff')
        for y in range(height):
            for x in range(width):
                block=((y//tile)*(pw//tile)+x//tile)*tile*tile
                block+=spread(x%tile)|(spread(y%tile)<<1)
                for sample in range(samples):
                    at=offset+layer*layer_stride+block*4*samples+sample*4
                    assert at+4<=len(expected) and at not in points
                    points.add(at)
                    expected[at:at+4]=pixel
    assert len(points)==width*height*layers*samples
    return bytes(expected),points,dict(layout='POT Morton',padded_width=pw,padded_height=ph,
                                      tile=tile,layer_stride=layer_stride,sample_cells=len(points))
