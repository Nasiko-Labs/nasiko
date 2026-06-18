use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "search_food",
                "description": "Search the USDA FoodData Central database for foods. Returns food names with IDs that can be used for detailed nutrition lookup.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Food item to search for (e.g. 'chicken breast', 'brown rice', 'avocado')"
                        },
                        "data_type": {
                            "type": "string",
                            "enum": ["Foundation", "SR Legacy", "Branded", "Survey"],
                            "description": "Filter by data type. 'Foundation' and 'SR Legacy' are whole/unbranded foods. 'Branded' is packaged foods."
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_nutrition",
                "description": "Get detailed nutrition facts for a specific food by its FDC ID. Returns calories, macros, vitamins, and minerals per 100g.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "fdc_id": {
                            "type": "integer",
                            "description": "FoodData Central ID (from search_food results)"
                        }
                    },
                    "required": ["fdc_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "open_food_facts",
                "description": "Search Open Food Facts for packaged/branded food products by name or barcode. Returns nutrition labels, ingredients, and Nutri-Score.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Product name or barcode to search for"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "compare_foods",
                "description": "Compare nutrition of multiple foods side by side. Takes a list of FDC IDs and returns a comparison table.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "fdc_ids": {
                            "type": "array",
                            "items": { "type": "integer" },
                            "description": "List of FoodData Central IDs to compare (2-5 items)"
                        }
                    },
                    "required": ["fdc_ids"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "search_food" => search_food(arguments).await,
        "get_nutrition" => get_nutrition(arguments).await,
        "open_food_facts" => open_food_facts(arguments).await,
        "compare_foods" => compare_foods(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

const USDA_API_KEY: &str = "DEMO_KEY";

async fn search_food(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let data_type = args["data_type"].as_str();

    let mut url = format!(
        "https://api.nal.usda.gov/fdc/v1/foods/search?api_key={}&query={}&pageSize=8",
        USDA_API_KEY,
        urlencode(query),
    );

    if let Some(dt) = data_type {
        url.push_str(&format!("&dataType={}", urlencode(dt)));
    }

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let foods = resp["foods"].as_array().ok_or("no results")?;

    if foods.is_empty() {
        return Ok(format!("No foods found for '{query}'."));
    }

    let results: Vec<String> = foods
        .iter()
        .map(|f| {
            let desc = f["description"].as_str().unwrap_or("?");
            let fdc_id = f["fdcId"].as_u64().unwrap_or(0);
            let data_type = f["dataType"].as_str().unwrap_or("?");
            let brand = f["brandOwner"].as_str().unwrap_or("");

            // Extract key nutrients from search results
            let nutrients = f["foodNutrients"].as_array();
            let mut kcal = String::from("?");
            let mut protein = String::from("?");
            let mut fat = String::from("?");
            let mut carbs = String::from("?");

            if let Some(nuts) = nutrients {
                for n in nuts {
                    let name = n["nutrientName"].as_str().unwrap_or("");
                    let val = n["value"].as_f64().unwrap_or(0.0);
                    match name {
                        "Energy" => kcal = format!("{val:.0}"),
                        "Protein" => protein = format!("{val:.1}"),
                        "Total lipid (fat)" => fat = format!("{val:.1}"),
                        "Carbohydrate, by difference" => carbs = format!("{val:.1}"),
                        _ => {}
                    }
                }
            }

            let brand_str = if brand.is_empty() { String::new() } else { format!(" ({brand})") };
            format!(
                "• **{desc}**{brand_str}\n  FDC ID: {fdc_id} | Type: {data_type}\n  Per 100g: {kcal} kcal | P: {protein}g | F: {fat}g | C: {carbs}g"
            )
        })
        .collect();

    Ok(format!("Found {} foods:\n\n{}", results.len(), results.join("\n\n")))
}

async fn get_nutrition(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let fdc_id = args["fdc_id"].as_u64().ok_or("missing 'fdc_id'")?;

    let url = format!(
        "https://api.nal.usda.gov/fdc/v1/food/{}?api_key={}",
        fdc_id, USDA_API_KEY,
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let desc = resp["description"].as_str().unwrap_or("Unknown food");
    let nutrients = resp["foodNutrients"].as_array().ok_or("no nutrient data")?;

    let mut output = format!("**{desc}** (per 100g)\n\n");

    let key_nutrients = [
        ("Energy", "kcal"),
        ("Protein", "g"),
        ("Total lipid (fat)", "g"),
        ("Carbohydrate, by difference", "g"),
        ("Fiber, total dietary", "g"),
        ("Sugars, total including NLEA", "g"),
        ("Calcium, Ca", "mg"),
        ("Iron, Fe", "mg"),
        ("Sodium, Na", "mg"),
        ("Potassium, K", "mg"),
        ("Vitamin C, total ascorbic acid", "mg"),
        ("Vitamin A, RAE", "µg"),
        ("Vitamin D (D2 + D3)", "µg"),
        ("Vitamin B-12", "µg"),
        ("Folate, total", "µg"),
        ("Magnesium, Mg", "mg"),
        ("Zinc, Zn", "mg"),
        ("Cholesterol", "mg"),
        ("Fatty acids, total saturated", "g"),
        ("Fatty acids, total monounsaturated", "g"),
        ("Fatty acids, total polyunsaturated", "g"),
    ];

    // Macros section
    output.push_str("**Macronutrients:**\n");
    for (name, unit) in &key_nutrients[..6] {
        if let Some(val) = find_nutrient(nutrients, name) {
            output.push_str(&format!("  {name}: {val:.1}{unit}\n"));
        }
    }

    // Vitamins & Minerals
    output.push_str("\n**Vitamins & Minerals:**\n");
    for (name, unit) in &key_nutrients[6..17] {
        if let Some(val) = find_nutrient(nutrients, name) {
            if val > 0.0 {
                output.push_str(&format!("  {name}: {val:.1}{unit}\n"));
            }
        }
    }

    // Fats breakdown
    output.push_str("\n**Fat Breakdown:**\n");
    for (name, unit) in &key_nutrients[17..] {
        if let Some(val) = find_nutrient(nutrients, name) {
            if val > 0.0 {
                let short_name = name.replace("Fatty acids, total ", "");
                output.push_str(&format!("  {short_name}: {val:.1}{unit}\n"));
            }
        }
    }

    Ok(output)
}

async fn open_food_facts(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;

    // Check if it looks like a barcode
    let url = if query.chars().all(|c| c.is_ascii_digit()) && query.len() >= 8 {
        format!("https://world.openfoodfacts.org/api/v2/product/{}.json", query)
    } else {
        format!(
            "https://world.openfoodfacts.org/cgi/search.pl?search_terms={}&json=1&page_size=5",
            urlencode(query),
        )
    };

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    // Single product (barcode lookup)
    if let Some(product) = resp.get("product") {
        return Ok(format_off_product(product));
    }

    // Search results
    let products = resp["products"].as_array().ok_or("no results")?;

    if products.is_empty() {
        return Ok(format!("No products found for '{query}'."));
    }

    let results: Vec<String> = products.iter().take(5).map(|p| format_off_product(p)).collect();
    Ok(results.join("\n\n---\n\n"))
}

async fn compare_foods(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let fdc_ids = args["fdc_ids"]
        .as_array()
        .ok_or("missing 'fdc_ids'")?
        .iter()
        .filter_map(|v| v.as_u64())
        .collect::<Vec<_>>();

    if fdc_ids.len() < 2 {
        return Err("need at least 2 FDC IDs to compare".into());
    }

    let mut foods: Vec<(String, Vec<(&str, f64)>)> = Vec::new();
    let compare_nutrients = ["Energy", "Protein", "Total lipid (fat)", "Carbohydrate, by difference", "Fiber, total dietary", "Sodium, Na", "Iron, Fe", "Calcium, Ca"];

    for id in &fdc_ids {
        let url = format!(
            "https://api.nal.usda.gov/fdc/v1/food/{}?api_key={}",
            id, USDA_API_KEY,
        );

        let resp = reqwest::get(&url)
            .await
            .map_err(|e| format!("request failed for {id}: {e}"))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("parse failed for {id}: {e}"))?;

        let desc = resp["description"].as_str().unwrap_or("?").to_string();
        let nutrients = resp["foodNutrients"].as_array();

        let mut values: Vec<(&str, f64)> = Vec::new();
        if let Some(nuts) = nutrients {
            for name in &compare_nutrients {
                let val = find_nutrient(nuts, name).unwrap_or(0.0);
                values.push((name, val));
            }
        }

        foods.push((desc, values));
    }

    // Build comparison table
    let mut output = String::from("**Nutrition Comparison (per 100g)**\n\n");
    output.push_str(&format!("| Nutrient | {} |\n", foods.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" | ")));
    output.push_str(&format!("|{}|\n", foods.iter().map(|_| "---").collect::<Vec<_>>().join("|")));

    for (i, name) in compare_nutrients.iter().enumerate() {
        let unit = match *name {
            "Energy" => "kcal",
            "Sodium, Na" | "Iron, Fe" | "Calcium, Ca" => "mg",
            _ => "g",
        };
        let vals: Vec<String> = foods.iter().map(|(_, v)| format!("{:.1}{unit}", v[i].1)).collect();
        output.push_str(&format!("| {name} | {} |\n", vals.join(" | ")));
    }

    Ok(output)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn find_nutrient(nutrients: &[serde_json::Value], name: &str) -> Option<f64> {
    nutrients.iter().find_map(|n| {
        let n_name = n["nutrient"]["name"].as_str()
            .or_else(|| n["nutrientName"].as_str())?;
        if n_name == name {
            n["amount"].as_f64().or_else(|| n["value"].as_f64())
        } else {
            None
        }
    })
}

fn format_off_product(product: &serde_json::Value) -> String {
    let name = product["product_name"].as_str().unwrap_or("Unknown product");
    let brand = product["brands"].as_str().unwrap_or("");
    let nutriscore = product["nutriscore_grade"].as_str().unwrap_or("?");
    let ingredients = product["ingredients_text"].as_str().unwrap_or("");

    let nutrients = &product["nutriments"];
    let kcal = nutrients["energy-kcal_100g"].as_f64().unwrap_or(0.0);
    let protein = nutrients["proteins_100g"].as_f64().unwrap_or(0.0);
    let fat = nutrients["fat_100g"].as_f64().unwrap_or(0.0);
    let carbs = nutrients["carbohydrates_100g"].as_f64().unwrap_or(0.0);
    let fiber = nutrients["fiber_100g"].as_f64().unwrap_or(0.0);
    let sugars = nutrients["sugars_100g"].as_f64().unwrap_or(0.0);
    let salt = nutrients["salt_100g"].as_f64().unwrap_or(0.0);
    let saturated = nutrients["saturated-fat_100g"].as_f64().unwrap_or(0.0);

    let brand_str = if brand.is_empty() { String::new() } else { format!(" — {brand}") };
    let ingredients_short = if ingredients.len() > 200 {
        format!("{}...", &ingredients[..200])
    } else {
        ingredients.to_string()
    };

    format!(
        "**{name}**{brand_str}\nNutri-Score: {nutriscore}\n\n\
        Per 100g:\n\
        • Energy: {kcal:.0} kcal\n\
        • Protein: {protein:.1}g\n\
        • Fat: {fat:.1}g (saturated: {saturated:.1}g)\n\
        • Carbs: {carbs:.1}g (sugars: {sugars:.1}g)\n\
        • Fiber: {fiber:.1}g\n\
        • Salt: {salt:.2}g\n\
        \nIngredients: {ingredients_short}"
    )
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            b' ' => "+".into(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}
