#!/usr/bin/env python3
"""Regenerate tests/fixtures/langdetect/corpus.json.

The expected values are langdetect 1.0.9's (the copy in the reference venv),
with `DetectorFactory.seed = 0` as beets sets it: the detected language, the
probabilities langdetect reports (rounded to 12 decimals), and the values
CPython's `random.Random(0)` draws in the detector's order. Run it with the
reference venv's Python:

    ~/.cache/beets-reference/venv/bin/python tools/gen-langdetect-corpus.py

The texts mix beets' lyrics test strings with lyrics-like text in every
language langdetect has a profile for, plus short, mixed-script, empty and
featureless input, and the normalization cases (Vietnamese decomposed
diacritics, Romanian comma-below letters, Farsi yeh, URLs and e-mail
addresses).
"""

import json
import os
import random
import unicodedata

from langdetect import DetectorFactory, LangDetectException, detect
from langdetect.detector_factory import detect_langs

DetectorFactory.seed = 0

TEXTS = {
    # beets' own lyrics test strings.
    "beets-rain": "sing in the rain every hour of the day",
    "beets-simple": "Some lyrics",
    "beets-synced": (
        "[00:00.00] Some synced lyrics\n[00:00.50]\n[00:01.00] Some more synced lyrics"
    ),
    "beets-translated": (
        "[00:00.00] Some synced lyrics / Quelques paroles synchronisées\n"
        "[00:00.50]\n"
        "[00:01.00] Some more synced lyrics / Quelques paroles plus synchronisées"
    ),
    "beets-lady-madonna": """\
The Beatles - Lady Madonna

Lady Madonna, children at your feet.
Wonder how you manage to make ends meet.
Who finds the money? When you pay the rent?
Did you think that money was heaven sent?
Friday night arrives without a suitcase.
Sunday morning creep in like a nun.
Monday's child has learned to tie his bootlace.
See how they run.
Lady Madonna, baby at your breast.
Wonder how you manage to feed the rest.
See how they run.""",
    "beets-refrain": """\
[Refrain: Doja Cat]
Hard for me to let you go (Let you go, let you go)
My body wouldn't let me hide it (Hide it)
[Chorus]
No matter what, I wouldn't fold (Wouldn't fold, wouldn't fold)
Ridin' through the thunder, lightnin'""",
    # Lyrics-like text in many languages.
    "en-long": """\
I woke up this morning with a song in my head, the kind of melody that stays
with you all day. The sun was coming through the window and the city was
already awake, buses rolling past the corner and the coffee shop opening its
doors. I put on my old jacket and walked down to the river, thinking about
everything that had changed since last summer and everything that never would.
There is a quiet kind of music in ordinary things: the sound of rain on a tin
roof, a train whistle in the distance, someone laughing in another room.""",
    "de": """\
Die Musik ist eine Kunstform, die seit Jahrtausenden die Menschen begleitet.
Auf dem Marktplatz spielte eine kleine Kapelle, und die Kinder tanzten im
Kreis. Am Abend saßen wir am Fluss und hörten den Wellen zu, während die
Sonne langsam hinter den Bergen verschwand. Es war ein wunderschöner Tag,
und niemand wollte nach Hause gehen.""",
    "fr": """\
La musique adoucit les mœurs, dit-on souvent. Ce matin, le marché était plein
de couleurs et de bruits, les gens discutaient devant les étals de fruits et
de légumes. Nous avons bu un café sur la terrasse en regardant passer les
voitures et les bicyclettes. Le soir, une petite fanfare a joué sur la place
du village, et tout le monde a dansé jusqu'à la nuit tombée.""",
    "es": """\
La música es el lenguaje del corazón, y en el pueblo todos saben cantar. Por
la mañana fuimos al mercado a comprar pan y fruta, y por la tarde nos sentamos
en la plaza a escuchar a los músicos. Los niños jugaban cerca de la fuente,
mientras el sol se ocultaba detrás de las montañas y las luces de las casas
se encendían una a una.""",
    "it": """\
La musica è una lingua che tutti capiscono, anche quando le parole sono
diverse. Stamattina il mercato era pieno di gente e di profumi, e abbiamo
comprato pane, pomodori e formaggio. Nel pomeriggio ci siamo seduti in piazza
ad ascoltare una piccola orchestra, mentre i bambini correvano intorno alla
fontana e il sole scendeva lentamente dietro le colline.""",
    "pt": """\
A música acompanha a nossa vida desde o primeiro dia, e no bairro todos
conhecem as canções antigas. De manhã fomos à feira comprar frutas e pão
fresco, e à tarde ficamos na praça ouvindo os músicos. As crianças brincavam
perto da fonte enquanto o sol se punha atrás das montanhas e as luzes das
casas se acendiam.""",
    "nl": """\
Muziek is een taal die iedereen begrijpt, ook als de woorden verschillen.
Vanmorgen was de markt vol mensen en geuren, en we kochten brood, kaas en
appels. In de middag zaten we op het plein te luisteren naar een klein
orkest, terwijl de kinderen bij de fontein speelden en de zon langzaam achter
de heuvels zakte.""",
    "sv": """\
Musiken följer oss genom livet, och i byn kan alla de gamla visorna. På
morgonen gick vi till marknaden för att köpa bröd och frukt, och på
eftermiddagen satt vi på torget och lyssnade på musikerna. Barnen lekte vid
fontänen medan solen sakta sjönk bakom bergen och ljusen tändes i fönstren.""",
    "da": """\
Musik er et sprog, som alle forstår, også når ordene er forskellige. I morges
var markedet fuld af mennesker og dufte, og vi købte brød, ost og æbler. Om
eftermiddagen sad vi på torvet og lyttede til musikerne, mens børnene legede
ved springvandet og solen langsomt gik ned bag bakkerne.""",
    "no": """\
Musikk følger oss gjennom hele livet, og i bygda kan alle de gamle sangene.
Om morgenen gikk vi til markedet for å kjøpe brød og frukt, og om ettermiddagen
satt vi på torget og hørte på musikerne. Barna lekte ved fontenen mens solen
sakte gikk ned bak fjellene og lysene ble tent i vinduene.""",
    "fi": """\
Musiikki seuraa meitä läpi elämän, ja kylässä kaikki osaavat vanhat laulut.
Aamulla menimme torille ostamaan leipää ja hedelmiä, ja iltapäivällä istuimme
torilla kuuntelemassa muusikoita. Lapset leikkivät suihkulähteen luona, kun
aurinko laski hitaasti vuorten taakse ja ikkunoihin syttyivät valot.""",
    "pl": """\
Muzyka towarzyszy nam przez całe życie, a w miasteczku wszyscy znają stare
piosenki. Rano poszliśmy na targ kupić chleb i owoce, a po południu
siedzieliśmy na rynku i słuchaliśmy muzyków. Dzieci bawiły się przy fontannie,
gdy słońce powoli zachodziło za górami, a w oknach zapalały się światła.""",
    "cs": """\
Hudba nás provází celým životem a ve vesnici každý zná staré písně. Ráno
jsme šli na trh koupit chléb a ovoce a odpoledne jsme seděli na náměstí a
poslouchali hudebníky. Děti si hrály u kašny, zatímco slunce pomalu zapadalo
za horami a v oknech se rozsvěcela světla.""",
    "sk": """\
Hudba nás sprevádza celým životom a v dedine každý pozná staré piesne. Ráno
sme išli na trh kúpiť chlieb a ovocie a popoludní sme sedeli na námestí a
počúvali hudobníkov. Deti sa hrali pri fontáne, kým slnko pomaly zapadalo za
horami a v oknách sa rozsvecovali svetlá.""",
    "sl": """\
Glasba nas spremlja vse življenje in na vasi vsi poznajo stare pesmi. Zjutraj
smo šli na trg kupit kruh in sadje, popoldne pa smo sedeli na trgu in
poslušali glasbenike. Otroci so se igrali pri vodnjaku, medtem ko je sonce
počasi zahajalo za gorami in so se v oknih prižigale luči.""",
    "hr": """\
Glazba nas prati cijeli život i u selu svi znaju stare pjesme. Ujutro smo
otišli na tržnicu kupiti kruh i voće, a poslijepodne smo sjedili na trgu i
slušali glazbenike. Djeca su se igrala kraj fontane dok je sunce polako
zalazilo za planinama i svjetla su se palila na prozorima.""",
    "hu": """\
A zene elkíséri az egész életünket, és a faluban mindenki ismeri a régi
dalokat. Reggel a piacra mentünk kenyeret és gyümölcsöt venni, délután pedig
a téren ültünk és hallgattuk a zenészeket. A gyerekek a szökőkútnál
játszottak, miközben a nap lassan lebukott a hegyek mögött.""",
    "ro": """\
Muzica ne însoțește toată viața, iar în sat toți cunosc cântecele vechi.
Dimineața am mers la piață să cumpărăm pâine și fructe, iar după-amiază am
stat în piațetă și i-am ascultat pe muzicieni. Copiii se jucau lângă fântână
în timp ce soarele apunea încet după munți.""",
    "bg": """\
Музиката ни съпътства през целия живот и в селото всички знаят старите
песни. Сутринта отидохме на пазара да купим хляб и плодове, а следобед
седяхме на площада и слушахме музикантите. Децата играеха до фонтана,
докато слънцето бавно залязваше зад планините.""",
    "ru": """\
Музыка сопровождает нас всю жизнь, и в деревне все знают старые песни.
Утром мы пошли на рынок купить хлеба и фруктов, а днём сидели на площади и
слушали музыкантов. Дети играли у фонтана, пока солнце медленно садилось
за горы, а в окнах зажигались огни.""",
    "uk": """\
Музика супроводжує нас усе життя, і в селі всі знають старі пісні. Уранці
ми пішли на ринок купити хліба та фруктів, а вдень сиділи на площі й
слухали музикантів. Діти гралися біля фонтана, поки сонце повільно
сідало за гори, а у вікнах засвічувалися вогні.""",
    "el": """\
Η μουσική μας συνοδεύει σε όλη μας τη ζωή και στο χωριό όλοι ξέρουν τα
παλιά τραγούδια. Το πρωί πήγαμε στην αγορά να αγοράσουμε ψωμί και φρούτα,
και το απόγευμα καθίσαμε στην πλατεία ακούγοντας τους μουσικούς. Τα παιδιά
έπαιζαν κοντά στη βρύση καθώς ο ήλιος έδυε αργά πίσω από τα βουνά.""",
    "tr": """\
Müzik hayatımız boyunca bize eşlik eder ve köyde herkes eski şarkıları
bilir. Sabah ekmek ve meyve almak için pazara gittik, öğleden sonra ise
meydanda oturup müzisyenleri dinledik. Çocuklar çeşmenin yanında oynarken
güneş yavaşça dağların arkasından batıyordu.""",
    "ar": """\
الموسيقى ترافقنا طوال حياتنا وفي القرية يعرف الجميع الأغاني القديمة. في
الصباح ذهبنا إلى السوق لنشتري الخبز والفواكه، وفي المساء جلسنا في الساحة
نستمع إلى الموسيقيين. كان الأطفال يلعبون قرب النافورة بينما كانت الشمس
تغرب ببطء خلف الجبال.""",
    "fa": """\
موسیقی در تمام زندگی همراه ماست و در روستا همه ترانه‌های قدیمی را می‌شناسند.
صبح برای خرید نان و میوه به بازار رفتیم و بعدازظهر در میدان نشستیم و به
نوازندگان گوش دادیم. بچه‌ها کنار فواره بازی می‌کردند و خورشید آرام‌آرام
پشت کوه‌ها فرو می‌رفت.""",
    "he": """\
מוזיקה מלווה אותנו כל החיים ובכפר כולם מכירים את השירים הישנים. בבוקר
הלכנו לשוק לקנות לחם ופירות, ואחר הצהריים ישבנו בכיכר והקשבנו לנגנים.
הילדים שיחקו ליד המזרקה בזמן שהשמש שקעה לאט מאחורי ההרים.""",
    "hi": """\
संगीत हमारे पूरे जीवन का साथ देता है और गाँव में सब पुराने गीत जानते हैं।
सुबह हम रोटी और फल खरीदने बाज़ार गए, और दोपहर में चौक पर बैठकर संगीतकारों
को सुना। बच्चे फव्वारे के पास खेल रहे थे जब सूरज धीरे-धीरे पहाड़ों के पीछे
छिप रहा था।""",
    "bn": """\
সংগীত আমাদের সারা জীবন সঙ্গে চলে এবং গ্রামে সবাই পুরনো গান জানে। সকালে আমরা
রুটি ও ফল কিনতে বাজারে গিয়েছিলাম, আর বিকেলে চত্বরে বসে সংগীতশিল্পীদের
গান শুনেছি। শিশুরা ঝরনার পাশে খেলছিল যখন সূর্য ধীরে ধীরে পাহাড়ের পিছনে
অস্ত যাচ্ছিল।""",
    "pa": """\
ਸੰਗੀਤ ਸਾਡੀ ਸਾਰੀ ਜ਼ਿੰਦਗੀ ਦਾ ਸਾਥ ਦਿੰਦਾ ਹੈ ਅਤੇ ਪਿੰਡ ਵਿੱਚ ਸਾਰੇ ਪੁਰਾਣੇ ਗੀਤ
ਜਾਣਦੇ ਹਨ। ਸਵੇਰੇ ਅਸੀਂ ਰੋਟੀ ਅਤੇ ਫਲ ਖਰੀਦਣ ਬਾਜ਼ਾਰ ਗਏ, ਅਤੇ ਦੁਪਹਿਰ ਨੂੰ ਚੌਕ
ਵਿੱਚ ਬੈਠ ਕੇ ਸੰਗੀਤਕਾਰਾਂ ਨੂੰ ਸੁਣਿਆ। ਬੱਚੇ ਫੁਹਾਰੇ ਕੋਲ ਖੇਡ ਰਹੇ ਸਨ ਜਦੋਂ ਸੂਰਜ
ਧੀਰੇ-ਧੀਰੇ ਪਹਾੜਾਂ ਪਿੱਛੇ ਡੁੱਬ ਰਿਹਾ ਸੀ।""",
    "gu": """\
સંગીત આપણા આખા જીવન સાથે ચાલે છે અને ગામમાં બધા જૂનાં ગીતો જાણે છે.
સવારે અમે રોટલી અને ફળ ખરીદવા બજારમાં ગયા, અને બપોરે ચોકમાં બેસીને
સંગીતકારોને સાંભળ્યા. બાળકો ફુવારા પાસે રમતાં હતાં જ્યારે સૂરજ ધીમે ધીમે
પર્વતો પાછળ ડૂબતો હતો.""",
    "mr": """\
संगीत आपल्या संपूर्ण आयुष्यासोबत असते आणि गावात सर्वांना जुनी गाणी माहीत
आहेत. सकाळी आम्ही भाकरी आणि फळे खरेदी करायला बाजारात गेलो, आणि दुपारी
चौकात बसून संगीतकारांना ऐकले. मुले कारंजाजवळ खेळत होती आणि सूर्य हळूहळू
डोंगरांमागे मावळत होता.""",
    "ne": """\
संगीतले हाम्रो सारा जीवनसँगै हिँड्छ र गाउँमा सबैलाई पुराना गीतहरू थाहा छ।
बिहान हामी रोटी र फलफूल किन्न बजार गयौं, र दिउँसो चोकमा बसेर संगीतकारहरूको
गीत सुन्यौं। केटाकेटीहरू झरनाको छेउमा खेलिरहेका थिए जब सूर्य बिस्तारै पहाड
पछाडि अस्ताउँदै थियो।""",
    "ta": """\
இசை நம் வாழ்நாள் முழுவதும் நம்முடன் இருக்கிறது, கிராமத்தில் எல்லோருக்கும்
பழைய பாடல்கள் தெரியும். காலையில் ரொட்டியும் பழங்களும் வாங்க கடைக்குச்
சென்றோம், மதியம் சதுக்கத்தில் அமர்ந்து இசைக்கலைஞர்களைக் கேட்டோம்.
குழந்தைகள் நீரூற்றின் அருகில் விளையாடிக்கொண்டிருந்தனர், சூரியன் மெதுவாக
மலைகளுக்குப் பின்னால் மறைந்துகொண்டிருந்தது.""",
    "te": """\
సంగీతం మన జీవితాంతం మనతో ఉంటుంది, గ్రామంలో అందరికీ పాత పాటలు తెలుసు.
ఉదయం రొట్టె, పండ్లు కొనడానికి మార్కెట్కు వెళ్ళాము, మధ్యాహ్నం చౌక్లో కూర్చుని
సంగీతకారులను విన్నాము. పిల్లలు ఫవ్వారా దగ్గర ఆడుకుంటుండగా సూర్యుడు నెమ్మదిగా
కొండల వెనుక మునిగిపోతున్నాడు.""",
    "kn": """\
ಸಂಗೀತ ನಮ್ಮ ಬದುಕಿನುದ್ದಕ್ಕೂ ಜೊತೆಗಿರುತ್ತದೆ, ಹಳ್ಳಿಯಲ್ಲಿ ಎಲ್ಲರಿಗೂ ಹಳೆಯ ಹಾಡುಗಳು
ಗೊತ್ತು. ಬೆಳಗ್ಗೆ ರೊಟ್ಟಿ ಮತ್ತು ಹಣ್ಣುಗಳನ್ನು ಕೊಳ್ಳಲು ಮಾರುಕಟ್ಟೆಗೆ ಹೋದೆವು, ಮಧ್ಯಾಹ್ನ
ಚೌಕದಲ್ಲಿ ಕುಳಿತು ಸಂಗೀತಗಾರರನ್ನು ಕೇಳಿದೆವು. ಮಕ್ಕಳು ಕಾರಂಜಿ ಬಳಿ ಆಡುತ್ತಿದ್ದರು,
ಸೂರ್ಯನು ನಿಧಾನವಾಗಿ ಬೆಟ್ಟಗಳ ಹಿಂದೆ ಮುಳುಗುತ್ತಿದ್ದನು.""",
    "ml": """\
സംഗീതം നമ്മുടെ ജീവിതകാലം മുഴുവൻ കൂടെയുണ്ട്, ഗ്രാമത്തിൽ എല്ലാവർക്കും പഴയ
പാട്ടുകൾ അറിയാം. രാവിലെ റൊട്ടിയും പഴവും വാങ്ങാൻ ഞങ്ങൾ ചന്തയിൽ പോയി, ഉച്ചയ്ക്ക്
ചത്വരത്തിൽ ഇരുന്ന് സംഗീതജ്ഞരെ കേട്ടു. കുട്ടികൾ ജലധാരയ്ക്കരികിൽ കളിക്കുമ്പോൾ
സൂര്യൻ പതിയെ മലകൾക്ക് പിന്നിൽ അസ്തമിച്ചുകൊണ്ടിരുന്നു.""",
    "th": """\
ดนตรีอยู่กับเรามาตลอดชีวิต และในหมู่บ้านทุกคนรู้จักเพลงเก่า ๆ ตอนเช้าเราไปตลาด
ซื้อขนมปังและผลไม้ ตอนบ่ายเรานั่งที่ลานกลางหมู่บ้านฟังนักดนตรีเล่น เด็ก ๆ วิ่งเล่น
อยู่ใกล้ ๆ น้ำพุ ขณะที่ดวงอาทิตย์ค่อย ๆ ลับลงหลังภูเขา""",
    "ja": """\
音楽は私たちの人生にいつも寄り添っています。村の人はみんな古い歌を知っています。
朝はパンと果物を買いに市場へ行き、午後は広場に座って演奏を聞きました。子供たちが
噴水のそばで遊んでいる間、太陽はゆっくりと山の向こうに沈んでいきました。""",
    "ko": """\
음악은 우리 삶을 언제나 함께 합니다. 마을 사람들은 모두 오래된 노래를 알고
있습니다. 아침에는 빵과 과일을 사러 시장에 갔고, 오후에는 광장에 앉아 연주를
들었습니다. 아이들이 분수 옆에서 노는 동안 해는 천천히 산 뒤로 지고 있었습니다.""",
    "zh-cn": """\
音乐伴随着我们的一生，村里的每个人都熟悉那些古老的歌。早上我们去市场买面包和
水果，下午坐在广场上听音乐家演奏。孩子们在喷泉旁玩耍，太阳慢慢落到山后，家家
户户的灯也亮了起来。""",
    "zh-tw": """\
音樂伴隨著我們的一生，村裡的每個人都熟悉那些古老的歌。早上我們去市場買麵包和
水果，下午坐在廣場上聽音樂家演奏。孩子們在噴泉旁玩耍，太陽慢慢落到山後，家家
戶戶的燈也亮了起來。""",
    "vi": """\
Âm nhạc luôn đồng hành cùng chúng ta trong suốt cuộc đời, và trong làng ai cũng
biết những bài hát cũ. Buổi sáng chúng tôi đi chợ mua bánh mì và hoa quả, buổi
chiều ngồi ở quảng trường nghe các nhạc công chơi đàn. Bọn trẻ chơi đùa bên đài
phun nước trong khi mặt trời lặn dần sau những ngọn núi.""",
    "id": """\
Musik selalu menemani kita sepanjang hidup, dan di kampung semua orang tahu
lagu-lagu lama. Pagi ini kami pergi ke pasar membeli roti dan buah, sorenya
kami duduk di alun-alun mendengarkan para pemusik. Anak-anak bermain di dekat
air mancur sementara matahari perlahan turun di balik gunung.""",
    "tl": """\
Ang musika ay kasama natin sa buong buhay, at sa nayon alam ng lahat ang
mga lumang awit. Kaninang umaga pumunta kami sa palengke upang bumili ng
tinapay at prutas, at sa hapon umupo kami sa plaza at nakinig sa mga
musikero. Naglalaro ang mga bata malapit sa fountain habang dahan-dahang
lumulubog ang araw sa likod ng mga bundok.""",
    "sw": """\
Muziki huandamana nasi maisha yetu yote, na kijijini kila mtu anajua nyimbo za
zamani. Asubuhi tulikwenda sokoni kununua mkate na matunda, na mchana
tuliketi kwenye uwanja kuwapenda wanamuziki. Watoto walikuwa wakicheza karibu
na chemchemi wakati jua lilipokuwa likitua polepole nyuma ya milima.""",
    "so": """\
Muusiga waxa uu nala socdaa nolosheena oo dhan, oo tuulada qof walba waa
yaqaan heesaha hore. Aroortii waxaan u baxnay suuqa inaan iibsanno rooti iyo
furuuto, duhurnimadii waxaan fadhiisannay barxadda oo waxaan dhagaysannay
fannaanadii. Carruurtu waxay ku ciyaarayeen agagaarka il baxa iyo qorraxdu
si tartiib ah ayay ugu dhici jirtay buuraha gadaashood.""",
    "af": """\
Musiek vergesel ons ons hele lewe lank, en in die dorp ken almal die ou
liedjies. Vanoggend het ons na die mark gegaan om brood en vrugte te koop, en
vanmiddag het ons op die plein gesit en na die musikante geluister. Die kinders
het by die fontein gespeel terwyl die son stadig agter die berge gesak het.""",
    "ca": """\
La música ens acompanya tota la vida, i al poble tothom coneix les cançons
antigues. Al matí vam anar al mercat a comprar pa i fruita, i a la tarda ens
vam asseure a la plaça a escoltar els músics. Els nens jugaven a prop de la
font mentre el sol s'amagava lentament darrere les muntanyes.""",
    "cy": """\
Mae cerddoriaeth yn ein dilyn drwy gydol ein bywyd, ac yn y pentref mae pawb
yn gwybod yr hen ganeuon. Y bore yma aethon ni i'r farchnad i brynu bara a
ffrwythau, ac yn y prynhawn eisteddon ni yn y sgwâr yn gwrando ar y cerddorion.
Roedd y plant yn chwarae wrth y ffynnon wrth i'r haul fachlannu'n araf y tu ôl
i'r mynyddoedd.""",
    "et": """\
Muusika saadab meid kogu elu ja külas teavad kõik vanu laule. Hommikul
läksime turule leiba ja puuvilju ostma ning pärastlõunal istusime väljakul ja
kuulasime muusikuid. Lapsed mängisid purskkaevu juures, kui päike vajus
aeglaselt mägede taha.""",
    "lt": """\
Muzika mus lydi visą gyvenimą, o kaime visi žino senas dainas. Ryte nuėjome į
turgų nusipirkti duonos ir vaisių, o po pietų sėdėjome aikštėje ir klausėmės
muzikantų. Vaikai žaidė prie fontano, kol saulė lėtai leidosi už kalnų.""",
    "lv": """\
Mūzika mūs pavada visu mūžu, un ciematā visi zina vecās dziesmas. No rīta mēs
aizgājām uz tirgu nopirkt maizi un augļus, un pēcpusdienā sēdējām laukumā un
klausījāmies mūziķus. Bērni spēlējās pie strūklakas, kamēr saule lēnām
norietēja aiz kalniem.""",
    "mk": """\
Музиката не придружува целиот живот и во селото сите ги знаат старите песни.
Наутро отидовме на пазар да купиме леб и овошје, а попладне седевме на
плоштадот и ги слушавме музичарите. Децата играа покрај фонтаната додека
сонцето полека заоѓаше зад планините.""",
    "sq": """\
Muzika na shoqëron gjatë gjithë jetës dhe në fshat të gjithë i njohin këngët
e vjetra. Në mëngjes shkuam në treg të blinim bukë dhe fruta, ndërsa pasdite
u ulëm në shesh dhe dëgjuam muzikantët. Fëmijët luanin pranë shatërvanit
ndërsa dielli perëndonte ngadalë pas maleve.""",
    "ur": """\
موسیقی ہماری ساری زندگی کے ساتھ رہتی ہے اور گاؤں میں سب پرانے گیت جانتے ہیں۔
صبح ہم روٹی اور پھل خریدنے بازار گئے اور دوپہر کو چوک میں بیٹھ کر موسیقاروں
کو سنا۔ بچے فوارے کے پاس کھیل رہے تھے جب سورج آہستہ آہستہ پہاڑوں کے پیچھے
ڈوب رہا تھا۔""",
    # Short texts (langdetect's behavior on little input).
    "short-hello": "hello",
    "short-a": "a",
    "short-ab": "ab",
    "short-ok": "ok",
    "short-no": "no",
    "short-hi": "hi",
    "short-bonjour": "bonjour",
    "short-hallo": "Hallo",
    "short-ja": "ありがとう",
    "short-zh": "谢谢",
    "short-ru": "Привет",
    "short-ar": "مرحبا",
    # Featureless input.
    "empty": "",
    "spaces": "   ",
    "newlines": "\n\n\n",
    "digits": "1234567890",
    "digits-spaced": "123 456 789 000",
    "punct": "!!!???...,,,;;;:::",
    "brackets": "<>[]{}()/*-+=",
    "url-only": "https://example.com/a/b?c=d&e=f#g",
    "mail-only": "someone@example.com",
    "url-mail-text": (
        "Visit https://example.com/some/page?x=1&y=2 or write to "
        "info@example.org for the lyrics, the rain keeps falling on the city"
    ),
    # Uppercase: capitalized words are skipped as features.
    "upper-sentence": "THIS IS A TEST OF THE EMERGENCY BROADCAST SYSTEM",
    "upper-long": "HELLO WORLD THIS IS A LONG UPPERCASE TEXT WITHOUT FEATURES",
    "capitalized": "The Rain In Spain Stays Mainly On The Plain",
    # Mixed scripts (cleaning_text drops Latin when non-Latin dominates).
    "mixed-cjk": "ok 今天天气很好，我们一起去公园散步，看看花和树。明天会下雨吗？",
    "mixed-cyrillic": "Hello world :) Сегодня хорошая погода, и мы пошли гулять в парк.",
    "mixed-latin-cyrillic-english": (
        "The quick brown fox jumps over the lazy dog and then runs away into "
        "the forest. Привет, как дела?"
    ),
    # Normalization cases.
    "vietnamese-decomposed": unicodedata.normalize(
        "NFD", "Tiếng Việt là ngôn ngữ của người Việt Nam và được sử dụng rộng rãi."
    ),
    "romanian-comma": "Știința și tehnologia se dezvoltă repede în țară.",
    "romanian-cedilla": "Ştiinţa şi tehnologia se dezvoltă repede în ţară.",
    "farsi-yeh": "من نمی‌دانم که فردا چه اتفاقی خواهد افتاد، اما امیدوارم خوب باشد.",
    "japanese-kana": "あいうえおかきくけこさしすせそたちつてとなにぬねの",
    "korean-syllables": "안녕하세요 오늘 날씨가 정말 좋네요 우리 같이 산책할까요",
    "cjk-clusters": "国鉄の切符を買って、電車に乗って東京へ行きました。",
    # Longer than max_text_length (10000): only the first 10000 characters
    # are used.
    "long-english": (
        "The music was playing in the old house by the river, and the windows "
        "were open to the summer night. "
    )
    * 200,
}

RNG_RANDOM = 8
RNG_GAUSS = 8
RNG_CHOICES = 20


def main():
    corpus = []
    for name, text in TEXTS.items():
        try:
            language = detect(text)
        except LangDetectException:
            language = None
        try:
            probs = [[l.lang, round(l.prob, 12)] for l in detect_langs(text)]
        except LangDetectException:
            probs = None
        corpus.append(
            {"name": name, "text": text, "language": language, "probs": probs}
        )

    r = random.Random(0)
    rng = {"random": [r.random() for _ in range(RNG_RANDOM)]}
    r = random.Random(0)
    rng["gauss"] = [r.gauss(0.0, 1.0) for _ in range(RNG_GAUSS)]
    r = random.Random(0)
    rng["choice"] = [r.choice(list(range(n))) for n in [1, 2, 3, 5, 8, 13, 21] * 3]

    data = {"rng": rng, "texts": corpus}
    target = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "tests",
        "fixtures",
        "langdetect",
        "corpus.json",
    )
    os.makedirs(os.path.dirname(target), exist_ok=True)
    with open(target, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=1)
        f.write("\n")
    print(f"{len(corpus)} texts -> {target}")


if __name__ == "__main__":
    main()
