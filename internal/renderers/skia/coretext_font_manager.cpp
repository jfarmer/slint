// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

#include "include/core/SkFontMgr.h"
#include "include/ports/SkFontMgr_mac_ct.h"

extern "C" void *slint_skia_coretext_small_font_manager()
{
    auto seed = CTFontCreateUIFontForLanguage(kCTFontUIFontSystem, 12, nullptr);
    if (!seed)
        return nullptr;
    auto descriptor = CTFontCopyFontDescriptor(seed);
    CFRelease(seed);
    if (!descriptor)
        return nullptr;

    const void *values[] = { descriptor };
    auto descriptors = CFArrayCreate(nullptr, values, 1, &kCFTypeArrayCallBacks);
    CFRelease(descriptor);
    if (!descriptors)
        return nullptr;
    auto collection = CTFontCollectionCreateWithFontDescriptors(descriptors, nullptr);
    CFRelease(descriptors);
    if (!collection)
        return nullptr;

    // CoreText returns null for no matches; Skia 153 assumes a non-null array.
    auto matches = CTFontCollectionCreateMatchingFontDescriptors(collection);
    if (!matches) {
        CFRelease(collection);
        return nullptr;
    }
    const bool has_fonts = CFArrayGetCount(matches) > 0;
    CFRelease(matches);
    if (!has_fonts) {
        CFRelease(collection);
        return nullptr;
    }

    auto manager = SkFontMgr_New_CoreText(collection);
    CFRelease(collection);
    return manager.release();
}
