import * as m from "$shared/i18n/messages";
import icon1Password from "../icons/1password.png";
import iconAuthenticator from "../icons/authenticator.png";
import iconBitwarden from "../icons/bitwarden.png";
import iconColorZilla from "../icons/colorzilla.png";
import iconDarkReader from "../icons/dark-reader.png";
import iconDashlane from "../icons/dashlane.png";
import iconDeepL from "../icons/deepl.png";
import iconEnhancerForYouTube from "../icons/enhancer-for-youtube.png";
import iconEnpass from "../icons/enpass.png";
import iconEvernoteWebClipper from "../icons/evernote-web-clipper.png";
import iconGoogleDictionary from "../icons/google-dictionary.png";
import iconGoogleDocsOffline from "../icons/google-docs-offline.png";
import iconGoogleTranslate from "../icons/google-translate.png";
import iconGrammarly from "../icons/grammarly.png";
import iconHypothesis from "../icons/hypothesis.png";
import iconJSONFormatter from "../icons/json-formatter.png";
import iconKeePassXCBrowser from "../icons/keepassxc-browser.png";
import iconKeeper from "../icons/keeper.png";
import iconLanguageTool from "../icons/languagetool.png";
import iconLastPass from "../icons/lastpass.png";
import iconLoom from "../icons/loom.png";
import iconNordPass from "../icons/nordpass.png";
import iconNotionWebClipper from "../icons/notion-web-clipper.png";
import iconOctotree from "../icons/octotree.png";
import iconOneTab from "../icons/onetab.png";
import iconProtonPass from "../icons/proton-pass.png";
import iconRaindropIo from "../icons/raindrop-io.png";
import iconReactDeveloperTools from "../icons/react-developer-tools.png";
import iconRefinedGitHub from "../icons/refined-github.png";
import iconReturnYouTubeDislike from "../icons/return-youtube-dislike.png";
import iconRoboForm from "../icons/roboform.png";
import iconSessionBuddy from "../icons/session-buddy.png";
import iconSimplifyGmail from "../icons/simplify-gmail.png";
import iconSponsorBlock from "../icons/sponsorblock.png";
import iconStylus from "../icons/stylus.png";
import iconTampermonkey from "../icons/tampermonkey.png";
import iconTodoist from "../icons/todoist.png";
import iconUnhook from "../icons/unhook.png";
import iconVideoSpeedController from "../icons/video-speed-controller.png";
import iconVimium from "../icons/vimium.png";
import iconViolentmonkey from "../icons/violentmonkey.png";
import iconVueJsDevtools from "../icons/vue-js-devtools.png";
import iconWappalyzer from "../icons/wappalyzer.png";
import iconWaybackMachine from "../icons/wayback-machine.png";
import iconZoteroConnector from "../icons/zotero-connector.png";

/** Extensions verified to work well in Zephium: in daily use, and in the
 *  real-extension suite (`cargo xtask webext-suite`), which also checks the
 *  ones that act on pages really do. "Get" opens their Chrome Web Store page. */
export type CatalogEntry = {
  id: string;
  name: string;
  blurb: () => string;
  /** Its Web Store icon, bundled so the page makes no request for it. */
  icon: string;
};

export type CatalogGroup = {
  title: () => string;
  entries: CatalogEntry[];
};

export const catalog: CatalogGroup[] = [
  {
    title: m.webext_group_passwords,
    entries: [
      {
        id: "hdokiejnpimakedhajhdlcegeplioahd",
        name: "LastPass",
        blurb: m.webext_catalog_lastpass,
        icon: iconLastPass,
      },
      {
        id: "nngceckbapebfimnlniiiahkandclblb",
        name: "Bitwarden",
        blurb: m.webext_catalog_bitwarden,
        icon: iconBitwarden,
      },
      {
        id: "aeblfdkhhhdcdjpifhhbdiojplfjncoa",
        name: "1Password",
        blurb: m.webext_catalog_1password,
        icon: icon1Password,
      },
      {
        id: "bhghoamapcdpbohphigoooaddinpkbai",
        name: "Authenticator",
        blurb: m.webext_catalog_authenticator,
        icon: iconAuthenticator,
      },
      {
        id: "fdjamakpfbbddfjaooikfcpapjohcfmg",
        name: "Dashlane",
        blurb: m.webext_catalog_dashlane,
        icon: iconDashlane,
      },
      {
        id: "bfogiafebfohielmmehodmfbbebbbpei",
        name: "Keeper",
        blurb: m.webext_catalog_keeper,
        icon: iconKeeper,
      },
      {
        id: "eiaeiblijfjekdanodkjadfinkhbfgcd",
        name: "NordPass",
        blurb: m.webext_catalog_nordpass,
        icon: iconNordPass,
      },
      {
        id: "pnlccmojcmeohlpggmfnbbiapkmbliob",
        name: "RoboForm",
        blurb: m.webext_catalog_roboform,
        icon: iconRoboForm,
      },
      {
        id: "ghmbeldphafepmbegfdlkpapadhbakde",
        name: "Proton Pass",
        blurb: m.webext_catalog_proton_pass,
        icon: iconProtonPass,
      },
      {
        id: "kmcfomidfpdkfieipokbalgegidffkal",
        name: "Enpass",
        blurb: m.webext_catalog_enpass,
        icon: iconEnpass,
      },
      {
        id: "oboonakemofpalcgghocfoadofidjkkk",
        name: "KeePassXC-Browser",
        blurb: m.webext_catalog_keepassxc,
        icon: iconKeePassXCBrowser,
      },
    ],
  },
  {
    title: m.webext_group_language,
    entries: [
      {
        id: "kbfnbcaeplbcioakkpcpgfkobkghlhen",
        name: "Grammarly",
        blurb: m.webext_catalog_grammarly,
        icon: iconGrammarly,
      },
      {
        id: "aapbdbdomjkkjkaonfhkkikfgjllcleb",
        name: "Google Translate",
        blurb: m.webext_catalog_google_translate,
        icon: iconGoogleTranslate,
      },
      {
        id: "mgijmajocgfcbeboacabfgobmjgjcoja",
        name: "Google Dictionary",
        blurb: m.webext_catalog_google_dictionary,
        icon: iconGoogleDictionary,
      },
      {
        id: "cofdbpoegempjloogbagkncekinflcnj",
        name: "DeepL",
        blurb: m.webext_catalog_deepl,
        icon: iconDeepL,
      },
      {
        id: "oldceeleldhonbafppcapldpdifcinji",
        name: "LanguageTool",
        blurb: m.webext_catalog_languagetool,
        icon: iconLanguageTool,
      },
    ],
  },
  {
    title: m.webext_group_browsing,
    entries: [
      {
        id: "eimadpbcbfnmbkopoojfekhnkhdbieeh",
        name: "Dark Reader",
        blurb: m.webext_catalog_dark_reader,
        icon: iconDarkReader,
      },
      {
        id: "chphlpgkkbolifaimnlloiipkdnihall",
        name: "OneTab",
        blurb: m.webext_catalog_onetab,
        icon: iconOneTab,
      },
      {
        id: "fpnmgdkabkmnadcjpehmlllkndpkmiak",
        name: "Wayback Machine",
        blurb: m.webext_catalog_wayback_machine,
        icon: iconWaybackMachine,
      },
      {
        id: "dbepggeogbaibhgnhhndojpepiihcmeb",
        name: "Vimium",
        blurb: m.webext_catalog_vimium,
        icon: iconVimium,
      },
      {
        id: "edacconmaakjimmfgnblocblbcdcpbko",
        name: "Session Buddy",
        blurb: m.webext_catalog_session_buddy,
        icon: iconSessionBuddy,
      },
      {
        id: "clngdbkpkpeebahjckkjfobafhncgmne",
        name: "Stylus",
        blurb: m.webext_catalog_stylus,
        icon: iconStylus,
      },
    ],
  },
  {
    title: m.webext_group_video,
    entries: [
      {
        id: "nffaoalbilbmmfgbnbgppjihopabppdk",
        name: "Video Speed Controller",
        blurb: m.webext_catalog_video_speed,
        icon: iconVideoSpeedController,
      },
      {
        id: "gebbhagfogifgggkldgodflihgfeippi",
        name: "Return YouTube Dislike",
        blurb: m.webext_catalog_ryd,
        icon: iconReturnYouTubeDislike,
      },
      {
        id: "ponfpcnoihfmfllpaingbgckeeldkhle",
        name: "Enhancer for YouTube",
        blurb: m.webext_catalog_enhancer_youtube,
        icon: iconEnhancerForYouTube,
      },
      {
        id: "mnjggcdmjocbbbhaepdhchncahnbgone",
        name: "SponsorBlock",
        blurb: m.webext_catalog_sponsorblock,
        icon: iconSponsorBlock,
      },
      {
        id: "khncfooichmfjbepaaaebmommgaepoid",
        name: "Unhook",
        blurb: m.webext_catalog_unhook,
        icon: iconUnhook,
      },
    ],
  },
  {
    title: m.webext_group_saving,
    entries: [
      {
        id: "pioclpoplcdbaefihamjohnefbikjilc",
        name: "Evernote Web Clipper",
        blurb: m.webext_catalog_evernote,
        icon: iconEvernoteWebClipper,
      },
      {
        id: "knheggckgoiihginacbkhaalnibhilkk",
        name: "Notion Web Clipper",
        blurb: m.webext_catalog_notion,
        icon: iconNotionWebClipper,
      },
      {
        id: "ldgfbffkinooeloadekpmfoklnobpien",
        name: "Raindrop.io",
        blurb: m.webext_catalog_raindrop,
        icon: iconRaindropIo,
      },
      {
        id: "ekhagklcjbdpajgpjgmbionohlpdbjgc",
        name: "Zotero Connector",
        blurb: m.webext_catalog_zotero,
        icon: iconZoteroConnector,
      },
      {
        id: "bjfhmglciegochdpefhhlphglcehbmek",
        name: "Hypothesis",
        blurb: m.webext_catalog_hypothesis,
        icon: iconHypothesis,
      },
    ],
  },
  {
    title: m.webext_group_productivity,
    entries: [
      {
        id: "ghbmnnjooekpmoecnnnilnnbdlolhkhi",
        name: "Google Docs Offline",
        blurb: m.webext_catalog_google_docs_offline,
        icon: iconGoogleDocsOffline,
      },
      {
        id: "liecbddmkiiihnedobmlmillhodjkdmb",
        name: "Loom",
        blurb: m.webext_catalog_loom,
        icon: iconLoom,
      },
      {
        id: "jldhpllghnbhlbpcmnajkpdmadaolakh",
        name: "Todoist",
        blurb: m.webext_catalog_todoist,
        icon: iconTodoist,
      },
      {
        id: "pbmlfaiicoikhdbjagjbglnbfcbcojpj",
        name: "Simplify Gmail",
        blurb: m.webext_catalog_simplify_gmail,
        icon: iconSimplifyGmail,
      },
    ],
  },
  {
    title: m.webext_group_developers,
    entries: [
      {
        id: "dhdgffkkebhmkfjojejmpbldmpobfkfo",
        name: "Tampermonkey",
        blurb: m.webext_catalog_tampermonkey,
        icon: iconTampermonkey,
      },
      {
        id: "fmkadmapgofadopljbjfkapdkoienihi",
        name: "React Developer Tools",
        blurb: m.webext_catalog_react_devtools,
        icon: iconReactDeveloperTools,
      },
      {
        id: "bhlhnicpbhignbdhedgjhgdocnmhomnp",
        name: "ColorZilla",
        blurb: m.webext_catalog_colorzilla,
        icon: iconColorZilla,
      },
      {
        id: "bcjindcccaagfpapjjmafapmmgkkhgoa",
        name: "JSON Formatter",
        blurb: m.webext_catalog_json_formatter,
        icon: iconJSONFormatter,
      },
      {
        id: "gppongmhjkpfnbhagpmjfkannfbllamg",
        name: "Wappalyzer",
        blurb: m.webext_catalog_wappalyzer,
        icon: iconWappalyzer,
      },
      {
        id: "nhdogjmejiglipccpnnnanhbledajbpd",
        name: "Vue.js devtools",
        blurb: m.webext_catalog_vue_devtools,
        icon: iconVueJsDevtools,
      },
      {
        id: "jinjaccalgkegednnccohejagnlnfdag",
        name: "Violentmonkey",
        blurb: m.webext_catalog_violentmonkey,
        icon: iconViolentmonkey,
      },
      {
        id: "bkhaagjahfmjljalopjnoealnfndnagc",
        name: "Octotree",
        blurb: m.webext_catalog_octotree,
        icon: iconOctotree,
      },
      {
        id: "hlepfoohegkhhmjieoechaddaejaokhf",
        name: "Refined GitHub",
        blurb: m.webext_catalog_refined_github,
        icon: iconRefinedGitHub,
      },
    ],
  },
];

export const storeListing = (id: string) => `https://chromewebstore.google.com/detail/${id}`;
