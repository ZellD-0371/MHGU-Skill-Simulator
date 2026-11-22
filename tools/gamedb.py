"""Game data for the build tools (and the local preview): Database/mhxx_all_tables.json, next to this repository,
read into a small cache (tools/.gamedb_cache.json, not committed).

GameData fields: armor, jewels, trees, skills, item_names, game_items, jewel_game (see _build_cache).
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DB_FILE = ROOT.parent / 'Database' / 'mhxx_all_tables.json'
CACHE = Path(__file__).with_name('.gamedb_cache.json')
CACHE_VERSION = 6

LANGS = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko', 'de', 'fr', 'it', 'es']
# Category tabs of the skill picker, by tree number ranges (plugin generated/data.rs CATEGORIES)
CATEGORIES = [
    # Attack/element + old critical/in-combat attack related (artillery is used by both blademasters and gunners so it is here; evasion and guts also give attack bonuses)
    ('cat_attack', [(19, 19), (28, 34), (71, 78), (88, 89), (91, 92), (94, 102), (108, 108), (110, 110)]),
    ('cat_blade', [(35, 45), (90, 90)]),                         # + horn (hunting horn)
    ('cat_gunner', [(46, 70)]),
    ('cat_guard', [(2, 18), (20, 27), (103, 103), (109, 109)]),  # + guts, mind's eye
    ('cat_move', [(79, 87)]),
    ('cat_life', [(93, 93), (104, 107), (111, 111), (112, 138)]),  # Old utility + bomb boost, sense, command, rally, mounting, SP extension
    ('cat_special', [(139, 205)]),
]

# Deviant armor (rarity 11, upgraded from one piece up to G rank) is told apart by the 'Hunting Seal 1' item its recipe
# needs. Value = lowest rank where the first special permit quest (which gives 2 of that seal) can be taken:
#   low:  Redhelm, Snowbaron, Stonefist, Dreadqueen (hall HR2 / hall 1-2 stars)
#   high: Drilltusk, Silverwind, Crystalbeard, Deadeye, Dreadking, Thunderlord, Grimclaw, Hellblade (hall 4-6 stars)
#   G:    the six added in XX (Nightcloak, Rustrazor, Soulseer, Boltreaver, Elderfrost, Bloodbath: first quest G1)
# The Database files every permit quest as 'G Sp' with no rank, so the low/high split comes from the quest unlock
# conditions (game-cap.com questlist90, checked 2026-11-22). Search use: that rank and every rank above it.
# Coin armor, told apart by the coin item id (names change with the Korean patch). Arena coins: the 14 monster coins
# 2062-2075 and the Grandpaw coin 2815. Event (collaboration) coins: the rest.
ARENA_COINS = set(range(2062, 2076)) | {2815}
EVENT_COINS = {2808, 2812, 2817, 2818, 2824, 2829, 2835, 2838, 2851, 2853}

DEVIANT_SEAL1 = {
    1767: 1, 1782: 1, 1797: 1, 1812: 1,
    1827: 2, 1842: 2, 1857: 2, 1872: 2, 1887: 2, 1902: 2, 1917: 2, 1932: 2,
    1947: 3, 1952: 3, 1957: 3, 1962: 3, 1967: 3, 1972: 3,
}


def deviant_rank(item_ids):
    """Start rank (1 low, 2 high, 3 G) of deviant armor from its recipe item ids, 0 for other armor"""
    return min((DEVIANT_SEAL1[i] for i in item_ids if i in DEVIANT_SEAL1), default=0)


def _names(row, prefix):
    return [row.get(f'{prefix}_{i}') or '' for i in range(9)]


def _build_cache():
    db = json.loads(DB_FILE.read_text(encoding='utf-8'))
    armor_skills = {}
    for r in db['DB_SklTreetoAmr']:
        armor_skills.setdefault(r['Amr_ID'], []).append([r['SklTree_ID'], r['Pt']])
    armor_names = {r['Amr_ID']: _names(r, 'Amr_Name') for r in db['ID_Amr_Name']}
    item_names = {r['Itm_ID']: _names(r, 'Itm_Name') for r in db['ID_Itm_Name']}
    # DB item numbers differ from game item numbers. The game number is the Hex column of DB_Itm.
    game_id = {r['Itm_ID']: int(r['Hex'], 16) for r in db['DB_Itm']}
    mats = {}
    for r in db['DB_ItmtoAmr']:
        mats.setdefault(r['Amr_ID'], []).append([r['Itm_ID'], r['Qty']])
    skill_names = {r['Skl_ID']: _names(r, 'Skl_Name') for r in db['ID_Skl_Name']}
    out = {
        'version': CACHE_VERSION,
        'armor': {r['Amr_ID']: {
            'part': r['Part'] - 1, 'kind': r['BorG'], 'slots': r['Slot'], 'def': r['MaxDef'], 'rare': r['Rare'],
            'res': [r['Res_Fire'], r['Res_Water'], r['Res_Thunder'], r['Res_Ice'], r['Res_Dragon']],
            'mats': mats.get(r['Amr_ID'], []), 'price': r['Price'],
            'skills': armor_skills.get(r['Amr_ID'], []), 'names': armor_names[r['Amr_ID']],
        } for r in db['DB_Amr']},
        'jewels': {},
        'items': {i: item_names[i] for i in {m[0] for v in mats.values() for m in v} if i in item_names},
        'game_items': {i: game_id[i] for i in {m[0] for v in mats.values() for m in v} if i in game_id},
        # Decoration item id -> game item number (the equipment box stores decorations by game item number)
        'jewel_game': {r['Itm_ID']: game_id[r['Itm_ID']] for r in db['DB_Jew'] if r['Itm_ID'] in game_id},
        'trees': {r['SklTree_ID']: _names(r, 'SklTree_Name') for r in db['ID_SklTree_Name'] if r['SklTree_ID'] > 0},
        'skills': [{'id': r['Skl_ID'], 'tree': r['SklTree_ID'], 'pt': r['Pt'], 'names': skill_names[r['Skl_ID']]}
                   for r in db['DB_Skl']],
    }
    for r in db['DB_Jew']:
        out['jewels'].setdefault(r['Itm_ID'], {
            'size': r['Slot'], 'pos': [r['SklTree1_ID'], r['SklTree1_Pt']],
            'neg': [r['SklTree2_ID'] or 0, r['SklTree2_Pt'] or 0], 'names': item_names[r['Itm_ID']],
        })
    CACHE.write_text(json.dumps(out, ensure_ascii=False), encoding='utf-8')


def load_raw():
    """The cached tables (rebuilt when the database is newer or the cache format changed)"""
    if not CACHE.exists() or CACHE.stat().st_mtime < DB_FILE.stat().st_mtime:
        _build_cache()
    raw = json.loads(CACHE.read_text(encoding='utf-8'))
    if raw.get('version') != CACHE_VERSION:
        _build_cache()
        raw = json.loads(CACHE.read_text(encoding='utf-8'))
    return raw


class GameData:
    def __init__(self):
        raw = load_raw()
        self.armor = {int(k): v for k, v in raw['armor'].items()}
        self.jewels = {int(k): v for k, v in raw['jewels'].items()}
        self.trees = {int(k): v for k, v in raw['trees'].items()}
        self.item_names = {int(k): v for k, v in raw['items'].items()}
        self.game_items = {int(k): v for k, v in raw['game_items'].items()}
        self.jewel_game = {int(k): v for k, v in raw['jewel_game'].items()}
        self.skills = raw['skills']
