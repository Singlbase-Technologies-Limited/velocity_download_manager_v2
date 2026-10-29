import { AppWindow, Archive, Book, Code, Disc, FileText, Film, Folder, Image, Music, Package, Star, type LucideIcon } from 'lucide-react';
import type { CategoryIcon } from '../contract';

/** The picture for each icon a category may have (contract.ts lists them, from Rust). */
export const CATEGORY_ICONS: Record<CategoryIcon, LucideIcon> = {
  folder: Folder,
  archive: Archive,
  'file-text': FileText,
  music: Music,
  film: Film,
  'app-window': AppWindow,
  disc: Disc,
  image: Image,
  code: Code,
  book: Book,
  package: Package,
  star: Star,
};
