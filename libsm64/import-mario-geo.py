#!/usr/bin/env python3
import os
import shutil
import urllib.request

GEO_URL = "https://raw.githubusercontent.com/n64decomp/sm64/06ec56df7f951f88da05f468cdcacecba496145a/actors/mario/geo.inc.c"
MODEL_URL = "https://raw.githubusercontent.com/n64decomp/sm64/06ec56df7f951f88da05f468cdcacecba496145a/actors/mario/model.inc.c"

geo_inc_c_header = """
#include "../include/sm64.h"
#include "../include/types.h"
#include "../include/geo_commands.h"
#include "../game/rendering_graph_node.h"
#include "../shim.h"
#include "../game/object_stuff.h"
#include "../game/behavior_actions.h"
#include "model.inc.h"

#define SHADOW_CIRCLE_PLAYER 99
"""

geo_inc_c_footer = """
const GeoLayout mario_geo_libsm64[] = {
   GEO_SHADOW(SHADOW_CIRCLE_PLAYER, 0xB4, 100),
   GEO_OPEN_NODE(),
      GEO_ZBUFFER(1),
      GEO_OPEN_NODE(),
         GEO_SCALE(0x00, 16384),
         GEO_OPEN_NODE(),
            GEO_ASM(0, geo_mirror_mario_backface_culling),
            GEO_ASM(0, geo_mirror_mario_set_alpha),
            GEO_BRANCH(1, mario_geo_load_body),
            GEO_ASM(1, geo_mirror_mario_backface_culling),
         GEO_CLOSE_NODE(),
      GEO_CLOSE_NODE(),
   GEO_CLOSE_NODE(),
   GEO_END(),
};

void *mario_geo_ptr = (void*)mario_geo_libsm64;

"""

geo_inc_h = """
#pragma once

extern void *mario_geo_ptr;
"""

model_inc_h = """
#pragma once

#include "../include/types.h"
#include "../include/PR/gbi.h"
"""

def strip_model_data(model):
    """Takes the vertices and light colours out of the model code: the arrays stay, empty, and
    are filled from the player's ROM when the library starts (mario_model_from_rom). The
    decompilation notes each one's address in segment 4 above it, which is where it sits in the
    ROM's Mario block (the one his textures are read from)."""
    import re
    table = []

    def vertices(m):
        count = m.group(3).count("{{{")
        table.append("{%s, 0x%06X, %d, 0}" % (m.group(2), int(m.group(1), 16) & 0xFFFFFF, count))
        return "// %s\nstatic Vtx %s[%d];" % (m.group(1), m.group(2), count)

    def lights(m):
        table.append("{&%s, 0x%06X, 1, 1}" % (m.group(2), int(m.group(1), 16) & 0xFFFFFF))
        return "// %s\nstatic Lights1 %s;" % (m.group(1), m.group(2))

    model, n = re.subn(r"// (0x04[0-9A-Fa-f]{6})[^\n]*\nstatic const Vtx (\w+)\[\] = \{(.*?)\n\};", vertices, model, flags=re.S)
    model, m = re.subn(r"// (0x04[0-9A-Fa-f]{6})[^\n]*\nstatic const Lights1 (\w+) = gdSPDefLights1\((.*?)\);", lights, model, flags=re.S)
    # (two light groups nothing uses have no address: emptied all the same)
    model = re.sub(r"UNUSED static const Lights1 (\w+) = gdSPDefLights1\((.*?)\);", r"UNUSED static Lights1 \1;", model, flags=re.S)
    if "static const Vtx" in model or "static const Lights1" in model or n == 0 or m == 0:
        raise SystemExit("the model code isn't laid out as expected: some vertex or light data would be left in")
    return model + """

// where each empty array above is in the ROM's Mario block: offset, how many, and whether it's
// a light (24 bytes as they are) or vertices (16 bytes each, big-endian)
static const struct { void *to; unsigned int offset; unsigned short count; unsigned char light; } mario_rom_data[] = {
    %s
};

void mario_model_from_rom(const unsigned char *segment, unsigned int size)
{
    // (lights are copied as they are; vertices field by field, their positions are floats here)
    _Static_assert(sizeof(Lights1) == 24, "N64 layout");
    for (unsigned int i = 0; i < sizeof(mario_rom_data) / sizeof(mario_rom_data[0]); i++) {
        const unsigned char *from = segment + mario_rom_data[i].offset;
        unsigned int bytes = mario_rom_data[i].count * (mario_rom_data[i].light ? 24 : 16);
        if (mario_rom_data[i].offset + bytes > size)
            continue;
        if (mario_rom_data[i].light) {
            memcpy(mario_rom_data[i].to, from, 24);
            continue;
        }
        Vtx *v = mario_rom_data[i].to;
        for (unsigned int k = 0; k < mario_rom_data[i].count; k++, from += 16) {
            for (int a = 0; a < 3; a++)
                v[k].v.ob[a] = (short)(from[a * 2] << 8 | from[a * 2 + 1]);
            v[k].v.flag = (unsigned short)(from[6] << 8 | from[7]);
            v[k].v.tc[0] = (short)(from[8] << 8 | from[9]);
            v[k].v.tc[1] = (short)(from[10] << 8 | from[11]);
            memcpy(v[k].v.cn, from + 12, 4);
        }
    }
}
""" % ",\n    ".join(table)


def main():
    global model_inc_h

    print("Downloading " + GEO_URL)
    geo_inc_c = urllib.request.urlopen(GEO_URL).read().decode('utf8')
    print("Downloading " + MODEL_URL)
    model_inc_c = urllib.request.urlopen(MODEL_URL).read().decode('utf8')

    lines = model_inc_c.splitlines()

    skip = 0
    for i in range(len(lines)):
        if skip > 0:
            skip = skip - 1
            lines[i] = "//" + lines[i]
        elif lines[i].startswith("ALIGNED8 static const u8 mario_"):
            skip = 2
            lines[i] = "//" + lines[i]
        elif lines[i].startswith("const "):
            model_inc_h += "\nextern " + lines[i].replace(" = {", ";")

    lines.insert(0, "#include \"../../gfx_macros.h\"")
    lines.insert(0, "#include \"../../load_tex_data.h\"")
    lines.insert(0, "#include <string.h>")
    model_inc_c = strip_model_data("\n".join(lines))
    model_inc_h += "\nvoid mario_model_from_rom(const unsigned char *segment, unsigned int size);\n"


    shutil.rmtree("src/decomp/mario", ignore_errors=True)
    os.makedirs("src/decomp/mario", exist_ok=True)

    with open("src/decomp/mario/geo.inc.c", "w") as file:
        file.write(geo_inc_c_header + geo_inc_c + geo_inc_c_footer)

    with open("src/decomp/mario/model.inc.c", "w") as file:
        file.write(model_inc_c)

    with open("src/decomp/mario/model.inc.h", "w") as file:
        file.write(model_inc_h)

    with open("src/decomp/mario/geo.inc.h", "w") as file:
        file.write(geo_inc_h)

if __name__ == "__main__":
    main()
