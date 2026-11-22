"""Database/mhxx_all_tables.json -> data/simdata.bin (+ human-readable name TSV).

Only simdata.bin goes into the plugin/engine. The TSV is for PC benchmark output.

simdata.bin (little-endian)
  magic  8s  "MHSS0005"
  counts HHH  skills, armor, jewels
  skill  HHb x            skill_id, tree, threshold
  armor  HBBBBBB HH 4*(Hbx) 5b x   id, part(0..4), kind, sex, rare, slots, flags, def, maxdef, skills,
                                   Resistances (fire, water, thunder, ice, dragon)
         flags: 1 = needs arena coins, 2 = needs event (collaboration) coins,
                bits 2-3 = deviant armor start rank (1 low, 2 high, 3 G; 0 = not deviant)
  jewel  HBB Hbx Hbx      item_id, size, rank (1 low, 2 high, 3 G), positive(tree,pt), negative(tree,pt)
  charm  B                number of charm kinds, then per kind
         BBB x            second skill threshold, first candidate count, second candidate count
         n1 * HBB         tree, min, max
         n2 * HBB         tree, max magnitude when negative, max positive
         20 * BBB         slot roll thresholds for fill values 1-20
"""
import json
import struct
import sys
from collections import defaultdict
from pathlib import Path

import gamedb

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "Database" / "mhxx_all_tables.json"
OUT = Path(__file__).resolve().parents[1] / "data"


def main() -> int:
    db = json.loads(SRC.read_text(encoding="utf-8"))
    OUT.mkdir(exist_ok=True)

    armor_skills = defaultdict(list)
    for row in db["DB_SklTreetoAmr"]:
        armor_skills[row["Amr_ID"]].append((row["SklTree_ID"], row["Pt"]))

    # Armor using coins as materials, told apart by item id (gamedb.ARENA_COINS / EVENT_COINS)
    coin_flag = {i: 1 for i in gamedb.ARENA_COINS} | {i: 2 for i in gamedb.EVENT_COINS}
    armor_flags = defaultdict(int)
    armor_items = defaultdict(list)
    for row in db["DB_ItmtoAmr"]:
        armor_flags[row["Amr_ID"]] |= coin_flag.get(row["Itm_ID"], 0)
        armor_items[row["Amr_ID"]].append(row["Itm_ID"])
    # Deviant armor: start rank in bits 2-3. Rarity 11 is used by deviant armor only (the plugin relies on this).
    for row in db["DB_Amr"]:
        rank = gamedb.deviant_rank(armor_items[row["Amr_ID"]])
        if (rank > 0) != (row["Rare"] == 11):
            raise SystemExit(f"희귀도 11과 이명 방어구가 어긋남: {row['Amr_ID']} rare {row['Rare']} rank {rank}")
        armor_flags[row["Amr_ID"]] |= rank << 2

    blob = bytearray()
    skills = sorted(db["DB_Skl"], key=lambda r: r["Skl_ID"])
    armor = sorted(db["DB_Amr"], key=lambda r: r["Amr_ID"])

    # Different recipes of the same item have the same effect, so merge them.
    # Lowest rank where each material is obtainable. Special (title) ranks fold into high/G rank.
    fold = {1: 1, 2: 2, 3: 3, 10: 3, 11: 3, 12: 3}
    item_rank = {}
    for table in ("DB_ItmtoMon", "DB_ItmtoLoc"):
        for row in db[table]:
            rank = fold.get(row["Rank_ID"], 2)
            item_rank[row["Itm_ID"]] = min(item_rank.get(row["Itm_ID"], 9), rank)
    # Decoration materials with no monster/gathering row, ranked from other checked sources (2026-11-22). Quest
    # rewards are NOT used for every item: village 6-star 'Advanced:' quests reward high rank stones and would pull
    # many high rank decorations down to low rank. Items without a checked source stay 'low' (printed below).
    verified_rank = {
        17: 1, 20: 1,          # Demondrug, Armorskin: combine Catalyst + Might/Adamant Seed (all low)
        18: 1, 21: 1,          # Mega Demondrug, Mega Armorskin: combine Nourishing Extract + the above (low)
        33: 2,                 # Hunter's Drink: village 10-star rewards; combine Hyper Extract (hall high, from 6 stars) + Catalyst
        34: 1,                 # Catalyst: village 6-star reward; combine Bitterbug + Honey (low)
        189: 1,                # Field Horn: village 1-star reward
        300: 1,                # Blue Cutthroat: hall 3-star reward
        327: 1, 1663: 1,       # Toxic Kumori, Sinister Cloth: Palico gathering (low)
        328: 2, 1664: 2,       # Large Toxic Kumori, Sinister Darkcloth: Palico gathering (high)
        354: 2,                # Irregular Stoutbone: special permit rewards (folded to high like the permit ranks above)
        356: 1, 370: 1,        # Wyvern Fang, Silver Egg: village 2-star rewards
        1444: 2,               # Deviljho Saliva: village 10-star reward
        1598: 1,               # Nakarkos Skull: hall 3-star reward
        1697: 1,               # Commendation: village 6-star reward
        1698: 2,               # Commendation G: village 10-star reward
    }
    for item, rank in verified_rank.items():
        if item in item_rank:
            raise SystemExit(f"확인 등급표의 소재에 이미 출처가 있음: {item}")
        item_rank[item] = rank
    recipe = defaultdict(list)
    for row in db["DB_ItmtoJew"]:
        recipe[row["Jew_ID"]].append(row["Itm_ID"])

    jewels = {}
    guessed = set()
    for row in db["DB_Jew"]:
        # Rank of a recipe = the highest rank among its materials. Materials without source info (combinations etc.) count as low rank.
        rank = max([item_rank.get(i, 1) for i in recipe[row["Jew_ID"]]] or [1])
        if rank < 3 and any(i not in item_rank for i in recipe[row["Jew_ID"]]):
            guessed.add(row["Itm_ID"])
        pos = (row["SklTree1_ID"], row["SklTree1_Pt"])
        neg = (row["SklTree2_ID"] or 0, row["SklTree2_Pt"] or 0)
        if pos[1] <= 0 or neg[1] > 0:
            raise SystemExit(f"예상 밖의 장식품 구성: {row}")
        if neg[0] <= 0:
            neg = (0, 0)
        key = (row["Itm_ID"], row["Slot"], pos, neg)
        jewels[key] = min(jewels.get(key, 9), rank)     # With several recipes, the easiest one
    jewels = sorted(jewels.items())

    blob += struct.pack("<8sHHH", b"MHSS0005", len(skills), len(armor), len(jewels))
    for row in skills:
        blob += struct.pack("<HHbx", row["Skl_ID"], row["SklTree_ID"], row["Pt"])
    for row in armor:
        pts = armor_skills[row["Amr_ID"]]
        if len(pts) > 4:
            raise SystemExit(f"스킬이 4개를 넘는 방어구: {row['Amr_ID']}")
        blob += struct.pack(
            "<HBBBBBBHH",
            row["Amr_ID"], row["Part"] - 1, row["BorG"], row["MorF"],
            row["Rare"], row["Slot"], armor_flags[row["Amr_ID"]], row["Def"], row["MaxDef"],
        )
        for tree, pt in pts + [(0, 0)] * (4 - len(pts)):
            blob += struct.pack("<Hbx", tree, pt)
        blob += struct.pack("<5bx", row["Res_Fire"], row["Res_Water"], row["Res_Thunder"],
                            row["Res_Ice"], row["Res_Dragon"])
    for (item, size, pos, neg), rank in jewels:
        blob += struct.pack("<HBBHbxHbx", item, size, rank, pos[0], pos[1], neg[0], neg[1])
    charm_table = json.loads(Path(__file__).with_name("charm_table.json").read_text(encoding="utf-8"))
    blob += struct.pack("<B", len(charm_table["kinds"]))
    for kind in charm_table["kinds"]:
        blob += struct.pack("<BBBx", kind["second_threshold"], len(kind["first"]), len(kind["second"]))
        for tree, lo, hi in kind["first"] + kind["second"]:
            blob += struct.pack("<HBB", tree, lo, hi)
        for row in kind["slot_table"]:
            blob += struct.pack("<BBB", *row)
    (OUT / "simdata.bin").write_bytes(blob)

    def names(table, key, prefix):
        return {r[key]: (r[f"{prefix}_0"] or "", r[f"{prefix}_4"] or "") for r in db[table]}

    skill_names = names("ID_Skl_Name", "Skl_ID", "Skl_Name")
    tree_names = names("ID_SklTree_Name", "SklTree_ID", "SklTree_Name")
    armor_names = names("ID_Amr_Name", "Amr_ID", "Amr_Name")
    item_names = names("ID_Itm_Name", "Itm_ID", "Itm_Name")

    def tsv(name, rows):
        text = "\n".join("\t".join(str(c) for c in row) for row in rows) + "\n"
        (OUT / name).write_text(text, encoding="utf-8")

    tsv("skills.tsv", [(r["Skl_ID"], r["SklTree_ID"], r["Pt"], *skill_names[r["Skl_ID"]]) for r in skills])
    tsv("trees.tsv", [(k, *v) for k, v in sorted(tree_names.items()) if k > 0])
    tsv("armor.tsv", [(r["Amr_ID"], *armor_names[r["Amr_ID"]]) for r in armor])
    tsv("jewels.tsv", [(j[0][0], *item_names[j[0][0]]) for j in jewels])
    print("arena armor", sum(1 for v in armor_flags.values() if v & 1),
          "event-coin armor", sum(1 for v in armor_flags.values() if v & 2))
    print("deviant armor", {r: sum(1 for v in armor_flags.values() if v >> 2 & 3 == r) for r in (1, 2, 3)})
    print("jewel ranks", {r: sum(1 for j in jewels if j[1] == r) for r in (1, 2, 3)})
    # Below G rank, a recipe with an unchecked material may really need a higher rank
    print("jewels ranked with unchecked materials", len(guessed), ", ".join(item_names[i][1] for i in sorted(guessed)))

    print(f"skills {len(skills)}  armor {len(armor)}  jewels {len(jewels)}  -> {len(blob)} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
