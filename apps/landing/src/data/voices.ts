// What people have posted about Zeron, in their own words and spelling.
import type { ImageMetadata } from "astro";
import afortunado from "../assets/voices/afortunado124.jpg";
import dev from "../assets/voices/dev_shekhawat.jpg";
import franco from "../assets/voices/francoe114696.jpg";
import leander from "../assets/voices/leanderriefel.jpg";
import sahil from "../assets/voices/sahilcodex.jpg";
import seis from "../assets/voices/stlaurentgod.jpg";
import adam from "../assets/voices/thecto.jpg";

export interface Voice {
  text: string;
  name: string;
  handle: string;
  url: string;
  avatar: ImageMetadata;
}

export const LEAD_VOICE: Voice = {
  text: "Oh man, this is the best UI I’ve ever tried! For real. It feels really smooth. Typing, reading, everything. Really fast. Right now it’s my main UI for coding.",
  name: "Fran Escob",
  handle: "@FrancoE114696",
  url: "https://x.com/FrancoE114696/status/2102551301871501508",
  avatar: franco,
};

export const VOICES: Voice[] = [
  {
    text: "I’m getting emotional seeing a UI so buttery smooth in performance after using the Codex App on Windows all day.",
    name: "Leander",
    handle: "@leanderriefel",
    url: "https://x.com/leanderriefel/status/2088396806396809544",
    avatar: leander,
  },
  {
    text: "bro i tested it all night, and it feels so good, congratulations i have a new daily driver, excelent taste",
    name: "af0r",
    handle: "@afOrtunado124",
    url: "https://x.com/afOrtunado124/status/2102727775991869480",
    avatar: afortunado,
  },
  {
    text: "if Zed & Cursor had a baby",
    name: "adam",
    handle: "@theCTO",
    url: "https://x.com/theCTO/status/2088337024868889064",
    avatar: adam,
  },
  {
    text: "best thing to happen to my agents this past week ✨",
    name: "seis dios ☽",
    handle: "@stlaurentgod",
    url: "https://x.com/stlaurentgod/status/2102509074097258904",
    avatar: seis,
  },
  {
    text: "This feels refreshing! already loving it",
    name: "Dev",
    handle: "@dev_shekhawat",
    url: "https://x.com/dev_shekhawat/status/2102533565590462728",
    avatar: dev,
  },
  {
    text: "one of the best app i have ever used",
    name: "丂ﾑんﾉﾚ",
    handle: "@sahilcodex",
    url: "https://x.com/sahilcodex/status/2102602368512860598",
    avatar: sahil,
  },
];
