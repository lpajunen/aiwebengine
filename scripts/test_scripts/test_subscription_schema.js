// Test script to verify GraphQL subscription schema configuration
async function testSubscriptionSchema() {
    const baseUrl = process.env.BASE_URL || 'http://localhost:8080';
    
    const introspectionQuery = {
        query: `
            query IntrospectionQuery {
                __schema {
                    subscriptionType {
                        name
                        fields {
                            name
                            type {
                                name
                            }
                        }
                    }
                }
            }
        `
    };
    
    try {
        const response = await fetch(`${baseUrl}/graphql`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify(introspectionQuery)
        });
        
        const result = await response.json();
        
        if (result.errors) {
            console.error('GraphQL Errors:', result.errors);
            return;
        }
        
        if (result.data.__schema.subscriptionType) {
            console.log('✅ GraphQL subscription type is configured!');
            console.log('Subscription type name:', result.data.__schema.subscriptionType.name);
            console.log('Available subscription fields:');
            result.data.__schema.subscriptionType.fields.forEach(field => {
                console.log(`  - ${field.name}: ${field.type.name}`);
            });
        } else {
            console.log('❌ GraphQL subscription type is NOT configured');
        }
        
    } catch (error) {
        console.error('Request failed:', error.message);
    }
}

// Run the test
testSubscriptionSchema();