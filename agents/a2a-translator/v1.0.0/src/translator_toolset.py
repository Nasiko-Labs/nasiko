import requests
import asyncio
import json
import random
import time
from typing import Any
from urllib.parse import urlparse, quote
from pydantic import BaseModel
from bs4 import BeautifulSoup
from langdetect import detect, DetectorFactory


# Set seed for consistent language detection
DetectorFactory.seed = 0


class TranslationRequest(BaseModel):
    """Request model for translation"""
    text: str
    source_language: str | None = None
    target_language: str = "en"


class URLTranslationRequest(BaseModel):
    """Request model for URL translation"""
    url: str
    source_language: str | None = None
    target_language: str = "en"


class LanguageDetectionRequest(BaseModel):
    """Request model for language detection"""
    text: str | None = None
    url: str | None = None


class TranslationResult(BaseModel):
    """Translation result information"""
    original_text: str
    translated_text: str
    source_language: str
    target_language: str
    confidence: float | None = None


class LanguageDetectionResult(BaseModel):
    """Language detection result"""
    detected_language: str
    confidence: float | None = None
    text_sample: str


class TranslationResponse(BaseModel):
    """Base response model for translation operations"""
    status: str
    message: str
    error_message: str | None = None


class TextTranslationResponse(TranslationResponse):
    """Response model for text translation"""
    data: TranslationResult | None = None


class URLTranslationResponse(TranslationResponse):
    """Response model for URL translation"""
    data: TranslationResult | None = None
    url: str | None = None
    page_title: str | None = None


class LanguageDetectionResponse(TranslationResponse):
    """Response model for language detection"""
    data: LanguageDetectionResult | None = None


class TranslatorToolset:
    """Translation toolset for translating text and web content"""

    def __init__(self):
        self.session = requests.Session()
        self.session.headers.update({
            'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36'
        })
        
    def _translate_with_google(self, text: str, src_lang: str, dest_lang: str) -> tuple[str, str]:
        """Translate text using Google Translate API directly"""
        try:
            # Google Translate URL
            url = "https://translate.googleapis.com/translate_a/single"
            
            params = {
                'client': 'gtx',
                'sl': src_lang,
                'tl': dest_lang,
                'dt': 't',
                'q': text
            }
            
            response = self.session.get(url, params=params, timeout=10)
            response.raise_for_status()
            
            result = response.json()
            
            # Extract translated text
            translated_text = ''
            if result and len(result) > 0 and result[0]:
                for sentence in result[0]:
                    if sentence and len(sentence) > 0:
                        translated_text += sentence[0]
            
            # Extract detected source language
            detected_src = src_lang
            if len(result) > 2 and result[2]:
                detected_src = result[2]
            
            return translated_text.strip(), detected_src
            
        except Exception as e:
            raise Exception(f"Translation failed: {str(e)}")

    def _extract_text_from_url(self, url: str) -> tuple[str, str | None]:
        """Extract text content from a URL
        
        Args:
            url: The URL to extract text from
            
        Returns:
            Tuple of (extracted_text, page_title)
        """
        try:
            # Validate URL
            parsed = urlparse(url)
            if not parsed.scheme or not parsed.netloc:
                raise ValueError("Invalid URL format")
            
            response = self.session.get(url, timeout=10)
            response.raise_for_status()
            
            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Remove script and style elements
            for script in soup(["script", "style"]):
                script.decompose()
            
            # Get page title
            title = soup.find('title')
            page_title = title.get_text().strip() if title else None
            
            # Extract text from body or fallback to entire document
            body = soup.find('body')
            if body:
                text = body.get_text()
            else:
                text = soup.get_text()
            
            # Clean up text
            lines = (line.strip() for line in text.splitlines())
            chunks = (phrase.strip() for line in lines for phrase in line.split("  "))
            text = ' '.join(chunk for chunk in chunks if chunk)
            
            return text, page_title
            
        except Exception as e:
            raise Exception(f"Failed to extract text from URL: {str(e)}")

    def _detect_language(self, text: str) -> tuple[str, float]:
        """Detect language of given text
        
        Args:
            text: Text to detect language for
            
        Returns:
            Tuple of (language_code, confidence)
        """
        try:
            # Use a sample of text for detection (first 1000 chars)
            sample_text = text[:1000] if len(text) > 1000 else text
            detected_lang = detect(sample_text)
            return detected_lang, 0.9  # langdetect doesn't provide confidence scores
        except Exception:
            return "unknown", 0.0

    async def translate_text(
        self, 
        text: str, 
        target_language: str = "en", 
        source_language: str | None = None
    ) -> str:
        """Translate plain text from one language to another
        
        Args:
            text: Text to translate
            target_language: Target language code (default: 'en')
            source_language: Source language code (auto-detect if None)
            
        Returns:
            str: The translated text
        """
        try:
            if not text.strip():
                return "Error: Empty text provided for translation"
            
            # Auto-detect source language if not provided
            if source_language is None:
                detected_lang, confidence = self._detect_language(text)
                source_language = detected_lang
            
            # Perform translation in thread pool to avoid blocking
            def _translate():
                return self._translate_with_google(text, source_language, target_language)
            
            loop = asyncio.get_event_loop()
            translated_text, detected_src = await loop.run_in_executor(None, _translate)
            
            return translated_text
            
        except Exception as e:
            return f"Translation failed: {str(e)}"

    def translate_url(
        self, 
        url: str, 
        target_language: str = "en", 
        source_language: str | None = None
    ) -> str:
        """Extract and translate content from a web page URL
        
        Args:
            url: URL to extract and translate content from
            target_language: Target language code (default: 'en')
            source_language: Source language code (auto-detect if None)
            
        Returns:
            str: The translated text from the webpage
        """
        try:
            # Extract text from URL
            extracted_text, page_title = self._extract_text_from_url(url)
            
            if not extracted_text.strip():
                return "Error: No text content found on the webpage"
            
            # Limit text length for translation (take first 5000 characters)
            if len(extracted_text) > 5000:
                extracted_text = extracted_text[:5000] + "..."
            
            # Auto-detect source language if not provided
            if source_language is None:
                detected_lang, confidence = self._detect_language(extracted_text)
                source_language = detected_lang
            
            # Perform translation
            translated_text, detected_src = self._translate_with_google(
                extracted_text, 
                source_language, 
                target_language
            )
            
            return translated_text
            
        except Exception as e:
            return f"Failed to translate URL content: {str(e)}"

    def detect_language(
        self, 
        text: str | None = None, 
        url: str | None = None
    ) -> str:
        """Detect the language of given text or URL content
        
        Args:
            text: Text to detect language for (optional)
            url: URL to extract text from and detect language (optional)
            
        Returns:
            str: The detected language code
        """
        try:
            if text and url:
                return "Error: Please provide either text or URL, not both"
            
            if not text and not url:
                return "Error: Please provide either text or URL for language detection"
            
            # Extract text from URL if provided
            if url:
                extracted_text, _ = self._extract_text_from_url(url)
                text = extracted_text
            
            if not text.strip():
                return "Error: No text content available for language detection"
            
            # Detect language
            detected_lang, confidence = self._detect_language(text)
            
            return detected_lang
            
        except Exception as e:
            return f"Failed to detect language: {str(e)}"

    def get_tools(self) -> dict[str, Any]:
        """Return dictionary of available tools for OpenAI function calling"""
        return {
            'translate_text': self,
            'translate_url': self,
            'detect_language': self,
        }