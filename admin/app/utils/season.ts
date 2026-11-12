// 季節の帯（二十四節気・七十二候・和暦）。持ち主の判断: 太陽の位置を近似の式で求め、依存も通信もなしで決める。
// 太陽の黄経の近似（Meeus の低精度の式。誤差は0.01度ほどで、節気の日付はほぼ1日以内で合う）。
// 日付は日本時間で数え、その日のうちに次の候に入るなら、その日を次の候とする（暦の数え方に合わせる）。

/** 二十四節気（立春から）。en は英語の画面で出す訳。 */
export const SEKKI = [
  { ja: "立春", en: "Beginning of spring" },
  { ja: "雨水", en: "Rain water" },
  { ja: "啓蟄", en: "Awakening of insects" },
  { ja: "春分", en: "Spring equinox" },
  { ja: "清明", en: "Clear and bright" },
  { ja: "穀雨", en: "Grain rain" },
  { ja: "立夏", en: "Beginning of summer" },
  { ja: "小満", en: "Lesser fullness" },
  { ja: "芒種", en: "Grain in ear" },
  { ja: "夏至", en: "Summer solstice" },
  { ja: "小暑", en: "Lesser heat" },
  { ja: "大暑", en: "Greater heat" },
  { ja: "立秋", en: "Beginning of autumn" },
  { ja: "処暑", en: "End of heat" },
  { ja: "白露", en: "White dew" },
  { ja: "秋分", en: "Autumn equinox" },
  { ja: "寒露", en: "Cold dew" },
  { ja: "霜降", en: "Frost descends" },
  { ja: "立冬", en: "Beginning of winter" },
  { ja: "小雪", en: "Lesser snow" },
  { ja: "大雪", en: "Greater snow" },
  { ja: "冬至", en: "Winter solstice" },
  { ja: "小寒", en: "Lesser cold" },
  { ja: "大寒", en: "Greater cold" },
] as const;

/** 七十二候（本朝七十二候、立春の初候から）。 */
export const KOU = [
  { ja: "東風解凍", yomi: "はるかぜこおりをとく", en: "East wind melts the ice" },
  { ja: "黄鶯睍睆", yomi: "うぐいすなく", en: "Bush warblers sing" },
  { ja: "魚上氷", yomi: "うおこおりをいずる", en: "Fish rise through the ice" },
  { ja: "土脉潤起", yomi: "つちのしょううるおいおこる", en: "Rain moistens the soil" },
  { ja: "霞始靆", yomi: "かすみはじめてたなびく", en: "Mist starts to linger" },
  { ja: "草木萌動", yomi: "そうもくめばえいずる", en: "Grass and trees sprout" },
  { ja: "蟄虫啓戸", yomi: "すごもりむしとをひらく", en: "Hibernating insects surface" },
  { ja: "桃始笑", yomi: "ももはじめてさく", en: "Peach trees begin to bloom" },
  { ja: "菜虫化蝶", yomi: "なむしちょうとなる", en: "Caterpillars become butterflies" },
  { ja: "雀始巣", yomi: "すずめはじめてすくう", en: "Sparrows start to nest" },
  { ja: "櫻始開", yomi: "さくらはじめてひらく", en: "Cherry blossoms begin to bloom" },
  { ja: "雷乃発声", yomi: "かみなりすなわちこえをはっす", en: "Distant thunder" },
  { ja: "玄鳥至", yomi: "つばめきたる", en: "Swallows return" },
  { ja: "鴻雁北", yomi: "こうがんかえる", en: "Wild geese fly north" },
  { ja: "虹始見", yomi: "にじはじめてあらわる", en: "First rainbows" },
  { ja: "葭始生", yomi: "あしはじめてしょうず", en: "First reeds sprout" },
  { ja: "霜止出苗", yomi: "しもやんでなえいずる", en: "Last frost, rice seedlings grow" },
  { ja: "牡丹華", yomi: "ぼたんはなさく", en: "Peonies bloom" },
  { ja: "蛙始鳴", yomi: "かわずはじめてなく", en: "Frogs start singing" },
  { ja: "蚯蚓出", yomi: "みみずいずる", en: "Worms surface" },
  { ja: "竹笋生", yomi: "たけのこしょうず", en: "Bamboo shoots sprout" },
  { ja: "蚕起食桑", yomi: "かいこおきてくわをはむ", en: "Silkworms feast on mulberry leaves" },
  { ja: "紅花栄", yomi: "べにばなさかう", en: "Safflowers bloom" },
  { ja: "麦秋至", yomi: "むぎのときいたる", en: "Wheat ripens" },
  { ja: "蟷螂生", yomi: "かまきりしょうず", en: "Praying mantises hatch" },
  { ja: "腐草為螢", yomi: "くされたるくさほたるとなる", en: "Fireflies fly from the grass" },
  { ja: "梅子黄", yomi: "うめのみきばむ", en: "Plums turn yellow" },
  { ja: "乃東枯", yomi: "なつかれくさかるる", en: "Self-heal withers" },
  { ja: "菖蒲華", yomi: "あやめはなさく", en: "Irises bloom" },
  { ja: "半夏生", yomi: "はんげしょうず", en: "Crow-dipper sprouts" },
  { ja: "温風至", yomi: "あつかぜいたる", en: "Warm winds blow" },
  { ja: "蓮始開", yomi: "はすはじめてひらく", en: "First lotus blossoms" },
  { ja: "鷹乃学習", yomi: "たかすなわちわざをならう", en: "Young hawks learn to fly" },
  { ja: "桐始結花", yomi: "きりはじめてはなをむすぶ", en: "Paulownia trees produce seeds" },
  { ja: "土潤溽暑", yomi: "つちうるおうてむしあつし", en: "Earth is damp, air is humid" },
  { ja: "大雨時行", yomi: "たいうときどきふる", en: "Great rains sometimes fall" },
  { ja: "涼風至", yomi: "すずかぜいたる", en: "Cool winds blow" },
  { ja: "寒蝉鳴", yomi: "ひぐらしなく", en: "Evening cicadas sing" },
  { ja: "蒙霧升降", yomi: "ふかききりまとう", en: "Thick fog descends" },
  { ja: "綿柎開", yomi: "わたのはなしべひらく", en: "Cotton flowers bloom" },
  { ja: "天地始粛", yomi: "てんちはじめてさむし", en: "Heat starts to die down" },
  { ja: "禾乃登", yomi: "こくものすなわちみのる", en: "Rice ripens" },
  { ja: "草露白", yomi: "くさのつゆしろし", en: "Dew glistens white on grass" },
  { ja: "鶺鴒鳴", yomi: "せきれいなく", en: "Wagtails sing" },
  { ja: "玄鳥去", yomi: "つばめさる", en: "Swallows leave" },
  { ja: "雷乃収声", yomi: "かみなりすなわちこえをおさむ", en: "Thunder ceases" },
  { ja: "蟄虫坏戸", yomi: "むしかくれてとをふさぐ", en: "Insects hole up underground" },
  { ja: "水始涸", yomi: "みずはじめてかるる", en: "Farmers drain the fields" },
  { ja: "鴻雁来", yomi: "こうがんきたる", en: "Wild geese return" },
  { ja: "菊花開", yomi: "きくのはなひらく", en: "Chrysanthemums bloom" },
  { ja: "蟋蟀在戸", yomi: "きりぎりすとにあり", en: "Crickets chirp around the door" },
  { ja: "霜始降", yomi: "しもはじめてふる", en: "First frost" },
  { ja: "霎時施", yomi: "こさめときどきふる", en: "Light rains sometimes fall" },
  { ja: "楓蔦黄", yomi: "もみじつたきばむ", en: "Maple leaves and ivy turn yellow" },
  { ja: "山茶始開", yomi: "つばきはじめてひらく", en: "Camellias begin to bloom" },
  { ja: "地始凍", yomi: "ちはじめてこおる", en: "The land starts to freeze" },
  { ja: "金盞香", yomi: "きんせんかさく", en: "Daffodils bloom" },
  { ja: "虹蔵不見", yomi: "にじかくれてみえず", en: "Rainbows hide" },
  { ja: "朔風払葉", yomi: "きたかぜこのはをはらう", en: "North wind blows the leaves from the trees" },
  { ja: "橘始黄", yomi: "たちばなはじめてきばむ", en: "Tachibana citrus leaves start to turn yellow" },
  { ja: "閉塞成冬", yomi: "そらさむくふゆとなる", en: "Cold sets in, winter begins" },
  { ja: "熊蟄穴", yomi: "くまあなにこもる", en: "Bears start hibernating" },
  { ja: "鱖魚群", yomi: "さけのうおむらがる", en: "Salmon gather and swim upstream" },
  { ja: "乃東生", yomi: "なつかれくさしょうず", en: "Self-heal sprouts" },
  { ja: "麋角解", yomi: "さわしかのつのおつる", en: "Deer shed antlers" },
  { ja: "雪下出麦", yomi: "ゆきわたりてむぎのびる", en: "Wheat sprouts under the snow" },
  { ja: "芹乃栄", yomi: "せりすなわちさかう", en: "Parsley flourishes" },
  { ja: "水泉動", yomi: "しみずあたたかをふくむ", en: "Springs thaw" },
  { ja: "雉始雊", yomi: "きじはじめてなく", en: "Pheasants start to call" },
  { ja: "款冬華", yomi: "ふきのはなさく", en: "Butterburs bud" },
  { ja: "水沢腹堅", yomi: "さわみずこおりつめる", en: "Ice thickens on streams" },
  { ja: "鶏始乳", yomi: "にわとりはじめてとやにつく", en: "Hens start laying eggs" },
] as const;

/** 季節ごとの3色（飾り。春・夏・秋・冬）。 */
const SEASON_COLORS = [
  ["#d98ea0", "#9cbf6b", "#f2d47a"],
  ["#3f8fa6", "#6aa86f", "#d9b44a"],
  ["#8c3a2b", "#c27c2c", "#5e6b3a"],
  ["#4a6b8c", "#a8b4c4", "#7a5c84"],
] as const;

const JST_OFFSET_MS = 9 * 60 * 60 * 1000;

/** 太陽の視黄経（度、0〜360）。 */
export function solarLongitude(at: Date): number {
  const n = at.getTime() / 86_400_000 + 2440587.5 - 2451545.0;
  const rad = Math.PI / 180;
  const L = 280.46 + 0.9856474 * n;
  const g = (357.528 + 0.9856003 * n) * rad;
  const lambda = L + 1.915 * Math.sin(g) + 0.02 * Math.sin(2 * g);
  return ((lambda % 360) + 360) % 360;
}

export interface Season {
  sekki: (typeof SEKKI)[number];
  kou: (typeof KOU)[number];
  colors: readonly string[];
}

/** その日（日本時間）の節気と候。その日の終わり（日本時間の24時）の太陽の位置で決める。 */
export function seasonOf(date: Date): Season {
  const jst = new Date(date.getTime() + JST_OFFSET_MS);
  const endOfDayUtc = Date.UTC(jst.getUTCFullYear(), jst.getUTCMonth(), jst.getUTCDate() + 1) - JST_OFFSET_MS;
  const lambda = solarLongitude(new Date(endOfDayUtc - 1));
  // 立春は黄経315度。そこから5度ごとに候、15度ごとに節気
  const k = Math.floor((((lambda - 315) % 360) + 360) % 360 / 5);
  const s = Math.floor(k / 3);
  return { sekki: SEKKI[s]!, kou: KOU[k]!, colors: SEASON_COLORS[Math.floor(s / 6)]! };
}

const KANJI_DIGITS = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
/** 1〜99 を漢数字に（十一、二十 など）。 */
function kanjiNumber(n: number): string {
  if (n < 10) return KANJI_DIGITS[n]!;
  const tens = Math.floor(n / 10);
  const ones = n % 10;
  return `${tens === 1 ? "" : KANJI_DIGITS[tens]}十${ones === 0 ? "" : KANJI_DIGITS[ones]}`;
}

/** 和暦の日付（例: 令和八年 十一月十二日）。元年は「元年」。ブラウザに組み込みの和暦を使う。 */
export function wareki(date: Date): string {
  const parts = new Intl.DateTimeFormat("ja-JP-u-ca-japanese", {
    era: "long",
    year: "numeric",
    month: "numeric",
    day: "numeric",
    timeZone: "Asia/Tokyo",
  }).formatToParts(date);
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? "";
  const year = Number(get("year"));
  return `${get("era")}${year === 1 ? "元" : kanjiNumber(year)}年 ${kanjiNumber(Number(get("month")))}月${kanjiNumber(Number(get("day")))}日`;
}
